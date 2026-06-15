//! `RpcL1Client`: the real `L1Client` over a Solana JSON-RPC endpoint
//! (devnet). Synchronous (the trait is), using a small `ureq` transport so
//! we don't pull the whole Agave RPC client stack into the node.
//!
//! Instructions for the Anchor settlement / DA-log programs are built by
//! hand — an 8-byte `sha256("global:<method>")` discriminator followed by
//! the Borsh encoding of the args (which, for our fixed-size and `Vec<u8>`
//! arguments, is just little-endian fields). This keeps the node free of the
//! Anchor client dependency while matching the on-chain account layouts in
//! `programs/` exactly.

use std::collections::HashMap;
use std::time::Duration;

use base64::Engine;
use serde_json::{json, Value};
use solana_sdk::{
    hash::{hash, Hash},
    instruction::{AccountMeta, Instruction},
    pubkey::Pubkey,
    signature::Keypair,
    signer::Signer,
    transaction::Transaction,
};

use crate::batch::{self, BatchPayload};
use crate::client::{BatchRecord, DepositEvent, L1Client, L1Error};
use crate::recover::RecoveredBatch;

const CONFIG_SEED: &[u8] = b"config";
const VAULT_SEED: &[u8] = b"vault";
const BATCH_SEED: &[u8] = b"batch";

/// Associated-token-account program (derives a deterministic token account per
/// (owner, mint)). The settlement program creates these for the vault and
/// recipients on demand.
fn ata_program_id() -> Pubkey {
    "ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL"
        .parse()
        .expect("valid associated-token program id")
}

/// Derive the associated token account address for `owner`/`mint` under the
/// given token program (classic Token or Token-2022).
fn derive_ata(owner: &Pubkey, mint: &Pubkey, token_program: &Pubkey) -> Pubkey {
    Pubkey::find_program_address(
        &[owner.as_ref(), token_program.as_ref(), mint.as_ref()],
        &ata_program_id(),
    )
    .0
}

/// `sha256("<namespace>:<name>")[..8]` — Anchor's instruction (namespace
/// `global`) and event (`event`) discriminator.
fn discriminator(namespace: &str, name: &str) -> [u8; 8] {
    let digest = hash(format!("{namespace}:{name}").as_bytes());
    let mut out = [0u8; 8];
    out.copy_from_slice(&digest.to_bytes()[..8]);
    out
}

/// `ComputeBudgetInstruction::SetComputeUnitLimit(units)` — tag byte 2 then a
/// little-endian u32, addressed to the ComputeBudget program.
fn compute_unit_limit(units: u32) -> Instruction {
    let mut data = vec![2u8];
    data.extend_from_slice(&units.to_le_bytes());
    let program_id: Pubkey = "ComputeBudget111111111111111111111111111111"
        .parse()
        .expect("valid compute budget program id");
    Instruction {
        program_id,
        accounts: vec![],
        data,
    }
}

pub struct RpcL1Client {
    rpc_url: String,
    agent: ureq::Agent,
    authority: Keypair,
    settlement_program: Pubkey,
    da_log_program: Pubkey,
    config_pda: Pubkey,
    vault_pda: Pubkey,
}

impl RpcL1Client {
    pub fn new(
        rpc_url: String,
        authority: Keypair,
        settlement_program: Pubkey,
        da_log_program: Pubkey,
    ) -> Result<Self, L1Error> {
        let (config_pda, _) =
            Pubkey::find_program_address(&[CONFIG_SEED], &settlement_program);
        let (vault_pda, _) = Pubkey::find_program_address(&[VAULT_SEED], &settlement_program);
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(10))
            .timeout_read(Duration::from_secs(30))
            .build();
        Ok(Self {
            rpc_url,
            agent,
            authority,
            settlement_program,
            da_log_program,
            config_pda,
            vault_pda,
        })
    }

    /// Create the settlement `config` PDA if it does not exist yet, making
    /// the bridge authority the sequencer authority. Idempotent.
    pub fn ensure_initialized(&self) -> Result<(), L1Error> {
        if self.account_data(&self.config_pda)?.is_some() {
            return Ok(());
        }
        let signature = self.send_and_confirm(self.ix_initialize())?;
        tracing::info!(%signature, "initialized L1 settlement config");
        Ok(())
    }

    /// Deposit `amount` lamports of SOL into the L1 bridge vault, crediting
    /// `l2_recipient` on the L2 (the deposit watcher mints the matching ZUL).
    /// Signed + paid by the configured authority.
    pub fn deposit_sol(&self, amount: u64, l2_recipient: Pubkey) -> Result<String, L1Error> {
        let mut data = discriminator("global", "deposit_sol").to_vec();
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(l2_recipient.as_ref());
        let ix = Instruction {
            program_id: self.settlement_program,
            accounts: vec![
                AccountMeta::new(self.config_pda, false),
                AccountMeta::new(self.vault_pda, false),
                AccountMeta::new(self.authority.pubkey(), true),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::id(), false),
            ],
            data,
        };
        self.send_and_confirm(ix)
    }

    /// Deposit `amount` of an SPL token (`mint`, under `token_program`) into the
    /// per-mint vault ATA, crediting `l2_recipient` with the wrapped token on
    /// the L2. Signed + paid by the configured authority (which must hold the
    /// tokens). For ops/testing — end users deposit via the SDK/frontend.
    pub fn deposit_spl(
        &self,
        amount: u64,
        mint: Pubkey,
        token_program: Pubkey,
        l2_recipient: Pubkey,
    ) -> Result<String, L1Error> {
        let mut data = discriminator("global", "deposit_spl").to_vec();
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(l2_recipient.as_ref());
        let depositor = self.authority.pubkey();
        let ix = Instruction {
            program_id: self.settlement_program,
            accounts: vec![
                AccountMeta::new_readonly(self.config_pda, false),
                AccountMeta::new_readonly(self.vault_pda, false),
                AccountMeta::new_readonly(mint, false),
                AccountMeta::new(derive_ata(&depositor, &mint, &token_program), false),
                AccountMeta::new(derive_ata(&self.vault_pda, &mint, &token_program), false),
                AccountMeta::new(depositor, true),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(ata_program_id(), false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::id(), false),
            ],
            data,
        };
        self.send_and_confirm(ix)
    }

    /// Claim an SPL withdrawal on L1: prove the asset-bound commitment is in
    /// `batch_no`'s posted withdrawals root and release the tokens from the
    /// per-mint vault ATA to `recipient`'s ATA.
    pub fn claim_withdrawal_spl(
        &self,
        batch_no: u64,
        amount: u64,
        nonce: u64,
        recipient: Pubkey,
        mint: Pubkey,
        token_program: Pubkey,
        bitmap: [u8; 32],
        siblings: &[[u8; 32]],
    ) -> Result<String, L1Error> {
        let mut data = discriminator("global", "claim_withdrawal_spl").to_vec();
        data.extend_from_slice(&batch_no.to_le_bytes());
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&nonce.to_le_bytes());
        data.extend_from_slice(&bitmap);
        data.extend_from_slice(&(siblings.len() as u32).to_le_bytes());
        for sibling in siblings {
            data.extend_from_slice(sibling);
        }
        let (claim_pda, _) = Pubkey::find_program_address(
            &[b"claim", recipient.as_ref(), &nonce.to_le_bytes()],
            &self.settlement_program,
        );
        let ix = Instruction {
            program_id: self.settlement_program,
            accounts: vec![
                AccountMeta::new_readonly(self.config_pda, false),
                AccountMeta::new_readonly(self.batch_pda(batch_no), false),
                AccountMeta::new_readonly(self.vault_pda, false),
                AccountMeta::new_readonly(mint, false),
                AccountMeta::new(derive_ata(&self.vault_pda, &mint, &token_program), false),
                AccountMeta::new_readonly(recipient, false),
                AccountMeta::new(derive_ata(&recipient, &mint, &token_program), false),
                AccountMeta::new(claim_pda, false),
                AccountMeta::new(self.authority.pubkey(), true),
                AccountMeta::new_readonly(token_program, false),
                AccountMeta::new_readonly(ata_program_id(), false),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::id(), false),
            ],
            data,
        };
        // send_and_confirm already prepends a 1.4M CU limit — ample for the
        // depth-32 blake3 SMT verify plus the token transfer CPI.
        self.send_and_confirm(ix)
    }

    /// Claim a withdrawal on L1: prove the commitment is in `batch_no`'s posted
    /// state root and release the SOL from the vault to `recipient`.
    pub fn claim_withdrawal(
        &self,
        batch_no: u64,
        amount: u64,
        nonce: u64,
        recipient: Pubkey,
        bitmap: [u8; 32],
        siblings: &[[u8; 32]],
    ) -> Result<String, L1Error> {
        let mut data = discriminator("global", "claim_withdrawal").to_vec();
        data.extend_from_slice(&batch_no.to_le_bytes());
        data.extend_from_slice(&amount.to_le_bytes());
        data.extend_from_slice(&nonce.to_le_bytes());
        data.extend_from_slice(&bitmap);
        data.extend_from_slice(&(siblings.len() as u32).to_le_bytes());
        for sibling in siblings {
            data.extend_from_slice(sibling);
        }
        let (claim_pda, _) = Pubkey::find_program_address(
            &[b"claim", recipient.as_ref(), &nonce.to_le_bytes()],
            &self.settlement_program,
        );
        let ix = Instruction {
            program_id: self.settlement_program,
            accounts: vec![
                AccountMeta::new_readonly(self.config_pda, false),
                AccountMeta::new_readonly(self.batch_pda(batch_no), false),
                AccountMeta::new(self.vault_pda, false),
                AccountMeta::new(recipient, false),
                AccountMeta::new(claim_pda, false),
                AccountMeta::new(self.authority.pubkey(), true),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::id(), false),
            ],
            data,
        };
        self.send_and_confirm(ix)
    }

    /// The (newest) batch number whose posted withdrawals root equals
    /// `withdrawals_root`, if any — the batch a withdrawal proof claims against.
    pub fn find_batch_for_withdrawals_root(
        &self,
        withdrawals_root: &[u8; 32],
    ) -> Result<Option<u64>, L1Error> {
        let count = self.config_batch_count()?;
        for batch_no in (0..count).rev() {
            if &self.fetch_batch_record(batch_no)?.withdrawals_root == withdrawals_root {
                return Ok(Some(batch_no));
            }
        }
        Ok(None)
    }

    /// Lamport balance of the batch-posting authority — for low-balance
    /// backpressure so the batcher pauses instead of spamming failed posts.
    pub fn authority_balance(&self) -> Result<u64, L1Error> {
        let result = self.rpc(
            "getBalance",
            json!([self.authority.pubkey().to_string(), { "commitment": "confirmed" }]),
        )?;
        result
            .get("value")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| L1Error::Rpc("getBalance: missing value".into()))
    }

    /// Slot cursor to resume batching from: one past the last settled slot.
    /// L1 is the source of truth, so a restarted sequencer never re-posts.
    pub fn resume_cursor_slot(&self) -> Result<u64, L1Error> {
        let count = self.config_batch_count()?;
        if count == 0 {
            // Genesis is slot 0; the first producible block is slot 1.
            return Ok(1);
        }
        let latest_no = count - 1;
        let data = self
            .account_data(&self.batch_pda(latest_no))?
            .ok_or_else(|| L1Error::Rpc(format!("batch {latest_no} record missing on L1")))?;
        // Batch: disc(8) batch_no(8) first_slot(8) last_slot(8) ...
        if data.len() < 32 {
            return Err(L1Error::Rpc("batch account too small".into()));
        }
        let last_slot = u64::from_le_bytes(data[24..32].try_into().unwrap());
        Ok(last_slot + 1)
    }

    // ----- disaster recovery ---------------------------------------------

    /// Recover every settlement batch from L1: read each batch record and
    /// reassemble + DA-hash-verify its payload from the DA-log chunks.
    pub fn fetch_settlement_batches(&self) -> Result<Vec<RecoveredBatch>, L1Error> {
        let count = self.config_batch_count()?;
        if count == 0 {
            return Ok(Vec::new());
        }
        let mut chunks = self.fetch_all_da_chunks()?;
        let mut out = Vec::with_capacity(count as usize);
        for batch_no in 0..count {
            let record = self.fetch_batch_record(batch_no)?;
            let mut parts = chunks.remove(&batch_no).unwrap_or_default();
            parts.sort_by_key(|(index, _)| *index);
            let compressed: Vec<u8> = parts.into_iter().flat_map(|(_, data)| data).collect();
            if batch::da_hash(&compressed) != record.da_hash {
                return Err(L1Error::Rpc(format!(
                    "batch {batch_no}: DA reassembly hash mismatch (missing/corrupt chunks)"
                )));
            }
            let payload = BatchPayload::decompress(&compressed)
                .map_err(|e| L1Error::Rpc(format!("batch {batch_no}: decompress: {e}")))?;
            out.push(RecoveredBatch { record, payload });
        }
        Ok(out)
    }

    fn fetch_batch_record(&self, batch_no: u64) -> Result<BatchRecord, L1Error> {
        let data = self
            .account_data(&self.batch_pda(batch_no))?
            .ok_or_else(|| L1Error::Rpc(format!("batch {batch_no} record missing on L1")))?;
        let le8 = |o: usize| u64::from_le_bytes(data[o..o + 8].try_into().unwrap());
        let h32 = |o: usize| {
            let mut h = [0u8; 32];
            h.copy_from_slice(&data[o..o + 32]);
            h
        };
        // disc(8) batch_no(8) first(8) last(8) state_root(32) [withdrawals_root(32)]
        // da_hash(32) chunk_count(4). Batches posted before the withdrawals_root
        // field (132 bytes) lack it — parse the older 100-byte layout too.
        if data.len() >= 132 {
            Ok(BatchRecord {
                batch_no: le8(8),
                first_slot: le8(16),
                last_slot: le8(24),
                state_root: h32(32),
                withdrawals_root: h32(64),
                da_hash: h32(96),
                chunk_count: u32::from_le_bytes(data[128..132].try_into().unwrap()),
            })
        } else if data.len() >= 100 {
            Ok(BatchRecord {
                batch_no: le8(8),
                first_slot: le8(16),
                last_slot: le8(24),
                state_root: h32(32),
                withdrawals_root: [0u8; 32],
                da_hash: h32(64),
                chunk_count: u32::from_le_bytes(data[96..100].try_into().unwrap()),
            })
        } else {
            Err(L1Error::Rpc(format!("batch {batch_no}: record too small")))
        }
    }

    /// All DA chunks ever posted, grouped by batch: batch_no -> [(index, bytes)].
    fn fetch_all_da_chunks(&self) -> Result<HashMap<u64, Vec<(u32, Vec<u8>)>>, L1Error> {
        let disc = discriminator("global", "post_chunk");
        let da_log = self.da_log_program.to_string();
        let mut map: HashMap<u64, Vec<(u32, Vec<u8>)>> = HashMap::new();
        let mut before: Option<String> = None;
        loop {
            let mut opts = json!({ "limit": 1000, "commitment": "confirmed" });
            if let Some(b) = &before {
                opts["before"] = json!(b);
            }
            let entries = self
                .rpc("getSignaturesForAddress", json!([da_log, opts]))?
                .as_array()
                .cloned()
                .unwrap_or_default();
            if entries.is_empty() {
                break;
            }
            for entry in &entries {
                if entry.get("err").map(|e| !e.is_null()).unwrap_or(false) {
                    continue;
                }
                if let Some(sig) = entry.get("signature").and_then(|s| s.as_str()) {
                    if let Some((batch_no, index, data)) = self.fetch_post_chunk(sig, &disc)? {
                        map.entry(batch_no).or_default().push((index, data));
                    }
                }
            }
            let page = entries.len();
            before = entries
                .last()
                .and_then(|e| e.get("signature"))
                .and_then(|s| s.as_str())
                .map(|s| s.to_string());
            if page < 1000 {
                break;
            }
        }
        Ok(map)
    }

    /// Decode a single `post_chunk` instruction from a transaction.
    fn fetch_post_chunk(
        &self,
        signature: &str,
        disc: &[u8; 8],
    ) -> Result<Option<(u64, u32, Vec<u8>)>, L1Error> {
        let tx = self.rpc(
            "getTransaction",
            json!([
                signature,
                { "encoding": "json", "commitment": "confirmed", "maxSupportedTransactionVersion": 0 }
            ]),
        )?;
        let Some(message) = tx.get("transaction").and_then(|t| t.get("message")) else {
            return Ok(None);
        };
        let keys = message
            .get("accountKeys")
            .and_then(|k| k.as_array())
            .cloned()
            .unwrap_or_default();
        let da_log = self.da_log_program.to_string();
        let instructions = message
            .get("instructions")
            .and_then(|i| i.as_array())
            .cloned()
            .unwrap_or_default();
        for ix in &instructions {
            let pid_index = ix
                .get("programIdIndex")
                .and_then(|v| v.as_u64())
                .unwrap_or(u64::MAX) as usize;
            if keys.get(pid_index).and_then(|k| k.as_str()) != Some(da_log.as_str()) {
                continue;
            }
            let encoded = ix.get("data").and_then(|d| d.as_str()).unwrap_or("");
            let Ok(bytes) = bs58::decode(encoded).into_vec() else {
                continue;
            };
            // disc(8) batch_no(8) chunk_index(4) chunk_count(4) len(4) data
            if bytes.len() < 28 || bytes[..8] != disc[..] {
                continue;
            }
            let batch_no = u64::from_le_bytes(bytes[8..16].try_into().unwrap());
            let chunk_index = u32::from_le_bytes(bytes[16..20].try_into().unwrap());
            let len = u32::from_le_bytes(bytes[24..28].try_into().unwrap()) as usize;
            let start = 28;
            if start + len > bytes.len() {
                continue;
            }
            return Ok(Some((batch_no, chunk_index, bytes[start..start + len].to_vec())));
        }
        Ok(None)
    }

    // ----- JSON-RPC transport --------------------------------------------

    fn rpc(&self, method: &str, params: Value) -> Result<Value, L1Error> {
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let text = self
            .agent
            .post(&self.rpc_url)
            .set("Content-Type", "application/json")
            .send_string(&body.to_string())
            .map_err(|e| L1Error::Rpc(format!("{method}: {e}")))?
            .into_string()
            .map_err(|e| L1Error::Rpc(format!("{method}: reading response: {e}")))?;
        let value: Value = serde_json::from_str(&text)
            .map_err(|e| L1Error::Rpc(format!("{method}: bad JSON: {e}")))?;
        if let Some(error) = value.get("error") {
            return Err(L1Error::Rpc(format!("{method}: {error}")));
        }
        Ok(value.get("result").cloned().unwrap_or(Value::Null))
    }

    fn latest_blockhash(&self) -> Result<Hash, L1Error> {
        let result = self.rpc("getLatestBlockhash", json!([{ "commitment": "confirmed" }]))?;
        let s = result
            .get("value")
            .and_then(|v| v.get("blockhash"))
            .and_then(|b| b.as_str())
            .ok_or_else(|| L1Error::Rpc("getLatestBlockhash: missing blockhash".into()))?;
        s.parse::<Hash>()
            .map_err(|e| L1Error::Rpc(format!("bad blockhash {s}: {e}")))
    }

    /// base64-decoded account data, or `None` if the account does not exist.
    fn account_data(&self, pubkey: &Pubkey) -> Result<Option<Vec<u8>>, L1Error> {
        let result = self.rpc(
            "getAccountInfo",
            json!([pubkey.to_string(), { "encoding": "base64", "commitment": "confirmed" }]),
        )?;
        match result.get("value") {
            None | Some(Value::Null) => Ok(None),
            Some(value) => {
                let encoded = value
                    .get("data")
                    .and_then(|d| d.get(0))
                    .and_then(|s| s.as_str())
                    .ok_or_else(|| L1Error::Rpc("getAccountInfo: unexpected data shape".into()))?;
                let bytes = base64::engine::general_purpose::STANDARD
                    .decode(encoded)
                    .map_err(|e| L1Error::Rpc(format!("account data base64: {e}")))?;
                Ok(Some(bytes))
            }
        }
    }

    /// Sign, submit, and wait for confirmation; returns the signature. A
    /// compute-unit-limit instruction is prepended so the SMT-verifying
    /// `claim_withdrawal` (256 blake3 hashes) doesn't exceed the 200k default;
    /// it's harmless for the cheaper instructions.
    fn send_and_confirm(&self, instruction: Instruction) -> Result<String, L1Error> {
        let blockhash = self.latest_blockhash()?;
        let tx = Transaction::new_signed_with_payer(
            &[compute_unit_limit(1_400_000), instruction],
            Some(&self.authority.pubkey()),
            &[&self.authority],
            blockhash,
        );
        let wire = bincode::serialize(&tx)
            .map_err(|e| L1Error::Rpc(format!("serializing transaction: {e}")))?;
        let encoded = base64::engine::general_purpose::STANDARD.encode(&wire);
        let signature = self
            .rpc(
                "sendTransaction",
                json!([encoded, { "encoding": "base64", "preflightCommitment": "confirmed" }]),
            )?
            .as_str()
            .ok_or_else(|| L1Error::Rpc("sendTransaction: no signature".into()))?
            .to_string();
        self.confirm(&signature)?;
        Ok(signature)
    }

    fn confirm(&self, signature: &str) -> Result<(), L1Error> {
        for _ in 0..45 {
            std::thread::sleep(Duration::from_secs(1));
            let result = self.rpc(
                "getSignatureStatuses",
                json!([[signature], { "searchTransactionHistory": true }]),
            )?;
            let status = result.get("value").and_then(|v| v.get(0)).cloned();
            let Some(status) = status else { continue };
            if status.is_null() {
                continue;
            }
            if let Some(err) = status.get("err") {
                if !err.is_null() {
                    return Err(L1Error::TransactionFailed(format!("{signature}: {err}")));
                }
            }
            match status.get("confirmationStatus").and_then(|c| c.as_str()) {
                Some("confirmed") | Some("finalized") => return Ok(()),
                _ => continue,
            }
        }
        Err(L1Error::TransactionFailed(format!(
            "{signature}: not confirmed within timeout"
        )))
    }

    // ----- account derivation + instruction building ---------------------

    fn batch_pda(&self, batch_no: u64) -> Pubkey {
        Pubkey::find_program_address(
            &[BATCH_SEED, &batch_no.to_le_bytes()],
            &self.settlement_program,
        )
        .0
    }

    fn config_batch_count(&self) -> Result<u64, L1Error> {
        match self.account_data(&self.config_pda)? {
            None => Ok(0),
            Some(data) => {
                // Config: disc(8) authority(32) batch_count(8) ...
                if data.len() < 48 {
                    return Err(L1Error::Rpc("config account too small".into()));
                }
                Ok(u64::from_le_bytes(data[40..48].try_into().unwrap()))
            }
        }
    }

    fn ix_initialize(&self) -> Instruction {
        Instruction {
            program_id: self.settlement_program,
            accounts: vec![
                AccountMeta::new(self.config_pda, false),
                AccountMeta::new_readonly(self.vault_pda, false),
                AccountMeta::new(self.authority.pubkey(), true),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::id(), false),
            ],
            data: discriminator("global", "initialize").to_vec(),
        }
    }

    fn ix_post_batch(&self, record: &BatchRecord) -> Instruction {
        let mut data = discriminator("global", "post_batch").to_vec();
        data.extend_from_slice(&record.batch_no.to_le_bytes());
        data.extend_from_slice(&record.first_slot.to_le_bytes());
        data.extend_from_slice(&record.last_slot.to_le_bytes());
        data.extend_from_slice(&record.state_root);
        data.extend_from_slice(&record.withdrawals_root);
        data.extend_from_slice(&record.da_hash);
        data.extend_from_slice(&record.chunk_count.to_le_bytes());
        Instruction {
            program_id: self.settlement_program,
            accounts: vec![
                AccountMeta::new(self.config_pda, false),
                AccountMeta::new(self.batch_pda(record.batch_no), false),
                AccountMeta::new(self.authority.pubkey(), true),
                AccountMeta::new_readonly(solana_sdk_ids::system_program::id(), false),
            ],
            data,
        }
    }

    fn ix_post_chunk(
        &self,
        batch_no: u64,
        chunk_index: u32,
        chunk_count: u32,
        chunk: &[u8],
    ) -> Instruction {
        let mut data = discriminator("global", "post_chunk").to_vec();
        data.extend_from_slice(&batch_no.to_le_bytes());
        data.extend_from_slice(&chunk_index.to_le_bytes());
        data.extend_from_slice(&chunk_count.to_le_bytes());
        // Borsh `Vec<u8>`: u32 length prefix then the bytes.
        data.extend_from_slice(&(chunk.len() as u32).to_le_bytes());
        data.extend_from_slice(chunk);
        Instruction {
            program_id: self.da_log_program,
            accounts: vec![AccountMeta::new(self.authority.pubkey(), true)],
            data,
        }
    }

    /// Decode a `Deposited` event from one transaction's program logs.
    fn deposit_in_tx(&self, signature: &str) -> Result<Option<DepositEvent>, L1Error> {
        let tx = self.rpc(
            "getTransaction",
            json!([
                signature,
                { "encoding": "json", "commitment": "confirmed", "maxSupportedTransactionVersion": 0 }
            ]),
        )?;
        let logs = tx
            .get("meta")
            .and_then(|m| m.get("logMessages"))
            .and_then(|l| l.as_array())
            .cloned()
            .unwrap_or_default();
        for line in logs {
            let Some(encoded) = line.as_str().and_then(|s| s.strip_prefix("Program data: ")) else {
                continue;
            };
            let Ok(bytes) = base64::engine::general_purpose::STANDARD.decode(encoded.trim()) else {
                continue;
            };
            if let Some(deposit) = parse_deposited_event(&bytes, signature) {
                return Ok(Some(deposit));
            }
        }
        Ok(None)
    }
}

/// Decode an Anchor `Deposited` event from its `Program data:` payload bytes.
/// Borsh layout: disc(8) depositor(32) amount(8) l2_recipient(32)
///   mint: Option<Pubkey> (1-byte tag, +32 if Some) decimals(1).
/// Tolerates the pre-SPL 80-byte layout (native, 9 decimals).
fn parse_deposited_event(bytes: &[u8], signature: &str) -> Option<DepositEvent> {
    let disc = discriminator("event", "Deposited");
    if bytes.len() < 80 || bytes[..8] != disc {
        return None;
    }
    let amount = u64::from_le_bytes(bytes[40..48].try_into().unwrap());
    let l2_recipient = Pubkey::new_from_array(bytes[48..80].try_into().unwrap());
    let (mint, decimals) = match bytes.get(80) {
        None => (None, 9),
        Some(0) => (None, *bytes.get(81).unwrap_or(&9)),
        Some(1) if bytes.len() >= 114 => (
            Some(Pubkey::new_from_array(bytes[81..113].try_into().unwrap())),
            bytes[113],
        ),
        // Malformed tail: treat as native rather than dropping the deposit.
        _ => (None, 9),
    };
    Some(DepositEvent {
        l1_signature: signature.to_string(),
        mint,
        decimals,
        amount,
        l2_recipient,
    })
}

impl L1Client for RpcL1Client {
    fn post_da_chunk(
        &self,
        batch_no: u64,
        chunk_index: u32,
        chunk_count: u32,
        chunk: &[u8],
    ) -> Result<String, L1Error> {
        self.send_and_confirm(self.ix_post_chunk(batch_no, chunk_index, chunk_count, chunk))
    }

    fn post_batch_record(&self, record: &BatchRecord) -> Result<String, L1Error> {
        self.send_and_confirm(self.ix_post_batch(record))
    }

    fn latest_batch_no(&self) -> Result<Option<u64>, L1Error> {
        Ok(match self.config_batch_count()? {
            0 => None,
            count => Some(count - 1),
        })
    }

    fn fetch_deposits(&self, after: Option<&str>) -> Result<Vec<DepositEvent>, L1Error> {
        // Deposits write the config PDA, so its signature history lists them
        // (alongside batch posts, which carry no `Deposited` event).
        let mut opts = json!({ "limit": 100, "commitment": "confirmed" });
        if let Some(cursor) = after {
            opts["until"] = json!(cursor);
        }
        let result = self.rpc(
            "getSignaturesForAddress",
            json!([self.config_pda.to_string(), opts]),
        )?;
        let entries = result.as_array().cloned().unwrap_or_default();
        let mut deposits = Vec::new();
        // RPC returns newest-first; the bridge wants oldest-first.
        for entry in entries.iter().rev() {
            if entry.get("err").map(|e| !e.is_null()).unwrap_or(false) {
                continue;
            }
            let Some(signature) = entry.get("signature").and_then(|s| s.as_str()) else {
                continue;
            };
            if let Some(deposit) = self.deposit_in_tx(signature)? {
                deposits.push(deposit);
            }
        }
        Ok(deposits)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn anchor_discriminators_match_sha256_prefix() {
        // Regression guard: these are the exact bytes Anchor emits for the
        // settlement program. If solana's hash ever changed, this catches it.
        // Computed as sha256("global:post_batch")[..8].
        assert_eq!(
            discriminator("global", "post_batch"),
            {
                use sha2::{Digest, Sha256};
                let d = Sha256::digest(b"global:post_batch");
                let mut out = [0u8; 8];
                out.copy_from_slice(&d[..8]);
                out
            }
        );
    }

    fn deposited_payload(mint: Option<Pubkey>, decimals: u8, amount: u64, recipient: &Pubkey) -> Vec<u8> {
        let mut b = discriminator("event", "Deposited").to_vec();
        b.extend_from_slice(Pubkey::new_unique().as_ref()); // depositor
        b.extend_from_slice(&amount.to_le_bytes());
        b.extend_from_slice(recipient.as_ref());
        match mint {
            None => b.push(0),
            Some(m) => {
                b.push(1);
                b.extend_from_slice(m.as_ref());
            }
        }
        b.push(decimals);
        b
    }

    #[test]
    fn decodes_native_and_spl_deposit_events() {
        let recipient = Pubkey::new_unique();
        let mint = Pubkey::new_unique();

        let native = parse_deposited_event(&deposited_payload(None, 9, 500, &recipient), "sig").unwrap();
        assert_eq!(native.mint, None);
        assert_eq!(native.decimals, 9);
        assert_eq!(native.amount, 500);
        assert_eq!(native.l2_recipient, recipient);

        let spl = parse_deposited_event(&deposited_payload(Some(mint), 6, 1_000_000, &recipient), "sig").unwrap();
        assert_eq!(spl.mint, Some(mint));
        assert_eq!(spl.decimals, 6);
        assert_eq!(spl.amount, 1_000_000);

        // Legacy 80-byte layout (no mint/decimals tail) decodes as native.
        let mut legacy = deposited_payload(None, 9, 7, &recipient);
        legacy.truncate(80);
        let old = parse_deposited_event(&legacy, "sig").unwrap();
        assert_eq!((old.mint, old.decimals, old.amount), (None, 9, 7));

        // Wrong discriminator → not a deposit.
        let mut bad = deposited_payload(None, 9, 1, &recipient);
        bad[0] ^= 0xff;
        assert!(parse_deposited_event(&bad, "sig").is_none());
    }

    #[test]
    fn post_chunk_data_is_anchor_borsh() {
        let (settlement, _) = Pubkey::find_program_address(&[CONFIG_SEED], &Pubkey::new_unique());
        let _ = settlement;
        let client = RpcL1Client::new(
            "http://localhost".into(),
            Keypair::new(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        )
        .unwrap();
        let ix = client.ix_post_chunk(7, 1, 3, &[0xaa, 0xbb]);
        // disc(8) | batch_no(8) | chunk_index(4) | chunk_count(4) | len(4) | data
        assert_eq!(ix.data.len(), 8 + 8 + 4 + 4 + 4 + 2);
        assert_eq!(&ix.data[8..16], &7u64.to_le_bytes());
        assert_eq!(&ix.data[16..20], &1u32.to_le_bytes());
        assert_eq!(&ix.data[20..24], &3u32.to_le_bytes());
        assert_eq!(&ix.data[24..28], &2u32.to_le_bytes());
        assert_eq!(&ix.data[28..], &[0xaa, 0xbb]);
        assert_eq!(ix.program_id, client.da_log_program);
        assert!(ix.accounts[0].is_signer);
    }

    #[test]
    fn post_batch_accounts_and_layout() {
        let client = RpcL1Client::new(
            "http://localhost".into(),
            Keypair::new(),
            Pubkey::new_unique(),
            Pubkey::new_unique(),
        )
        .unwrap();
        let record = BatchRecord {
            batch_no: 2,
            first_slot: 10,
            last_slot: 20,
            state_root: [3u8; 32],
            withdrawals_root: [5u8; 32],
            da_hash: [4u8; 32],
            chunk_count: 5,
        };
        let ix = client.ix_post_batch(&record);
        // disc(8)+batch_no(8)+first(8)+last(8)+state(32)+withdrawals(32)+da(32)+chunks(4)
        assert_eq!(ix.data.len(), 8 + 8 + 8 + 8 + 32 + 32 + 32 + 4);
        assert_eq!(ix.accounts.len(), 4);
        assert_eq!(ix.accounts[0].pubkey, client.config_pda);
        assert_eq!(ix.accounts[1].pubkey, client.batch_pda(2));
        assert!(ix.accounts[2].is_signer);
        assert_eq!(ix.accounts[3].pubkey, solana_sdk_ids::system_program::id());
    }
}
