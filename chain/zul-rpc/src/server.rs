//! Solana-compatible JSON-RPC server over HTTP.
//!
//! Implements the subset of methods that `@solana/web3.js`, wallet-adapter,
//! and the `solana` CLI need to operate against the L2. Parameters follow
//! Solana's positional-array convention `[target, { config }]`.

use base64::Engine;
use jsonrpsee::server::{RpcModule, Server, ServerConfig, ServerHandle};
use jsonrpsee::types::error::ErrorObjectOwned;
use serde_json::{json, Value};
use solana_sdk::{
    pubkey::Pubkey, signature::Signature, transaction::VersionedTransaction,
};
use std::net::SocketAddr;
use std::str::FromStr;
use std::sync::Arc;
use tokio::sync::broadcast;

use crate::backend::RpcBackend;
use crate::encode;

fn invalid_params(msg: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(-32602, msg.into(), None::<()>)
}

fn internal(msg: impl Into<String>) -> ErrorObjectOwned {
    ErrorObjectOwned::owned(-32603, msg.into(), None::<()>)
}

fn parse_pubkey(value: Option<&Value>) -> Result<Pubkey, ErrorObjectOwned> {
    let s = value
        .and_then(|v| v.as_str())
        .ok_or_else(|| invalid_params("expected a base58 pubkey string"))?;
    Pubkey::from_str(s).map_err(|_| invalid_params("invalid pubkey"))
}

/// Parse an optional base58 signature from `config[key]`.
fn parse_opt_signature(
    config: Option<&Value>,
    key: &str,
) -> Result<Option<Signature>, ErrorObjectOwned> {
    match config.and_then(|c| c.get(key)).and_then(|v| v.as_str()) {
        Some(s) => Signature::from_str(s)
            .map(Some)
            .map_err(|_| invalid_params(format!("invalid {key} signature"))),
        None => Ok(None),
    }
}

/// `getProgramAccounts` filters: `{ dataSize }` and
/// `{ memcmp: { offset, bytes } }` (bytes base58). True if all pass.
fn passes_filters(data: &[u8], filters: &[Value]) -> bool {
    for f in filters {
        if let Some(size) = f.get("dataSize").and_then(|v| v.as_u64()) {
            if data.len() as u64 != size {
                return false;
            }
        } else if let Some(mc) = f.get("memcmp") {
            let offset = mc.get("offset").and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let bytes = mc
                .get("bytes")
                .and_then(|v| v.as_str())
                .and_then(|s| bs58::decode(s).into_vec().ok())
                .unwrap_or_default();
            let end = offset.saturating_add(bytes.len());
            if end > data.len() || data[offset..end] != bytes[..] {
                return false;
            }
        }
    }
    true
}

/// Decode a transaction from a `sendTransaction`-style string param.
///
/// `encoding` is a hint, not a contract: web3.js and the CLI disagree on the
/// default (base64 vs base58) across versions, so we try the hinted encoding
/// first and fall back to the other. Only a string that decodes AND
/// deserializes into a transaction is accepted.
fn decode_transaction(
    value: Option<&Value>,
    encoding: &str,
) -> Result<VersionedTransaction, ErrorObjectOwned> {
    let s = value
        .and_then(|v| v.as_str())
        .ok_or_else(|| invalid_params("expected an encoded transaction string"))?;

    let try_base64 = || base64::engine::general_purpose::STANDARD.decode(s).ok();
    let try_base58 = || bs58::decode(s).into_vec().ok();
    let candidates = if encoding == "base58" {
        [try_base58(), try_base64()]
    } else {
        [try_base64(), try_base58()]
    };

    for bytes in candidates.into_iter().flatten() {
        if let Ok(tx) = bincode::deserialize::<VersionedTransaction>(&bytes) {
            return Ok(tx);
        }
    }
    Err(invalid_params("undeserializable transaction"))
}

fn config_str<'a>(params: &'a [Value], key: &str, default: &'a str) -> String {
    params
        .get(1)
        .and_then(|c| c.get(key))
        .and_then(|v| v.as_str())
        .unwrap_or(default)
        .to_string()
}

fn params_array(raw: Value) -> Vec<Value> {
    match raw {
        Value::Array(a) => a,
        Value::Null => vec![],
        other => vec![other],
    }
}

/// Build the RPC module with all method handlers bound to `backend`.
///
/// `faucet_enabled` gates the `requestAirdrop` method: on a value-bearing
/// chain the faucet is off, and the method is not exposed at all.
pub fn build_module<B: RpcBackend>(backend: Arc<B>, faucet_enabled: bool) -> RpcModule<Arc<B>> {
    let mut module = RpcModule::new(backend);

    module
        .register_method("getHealth", |_params, ctx, _ext| {
            if ctx.is_healthy() {
                Ok::<_, ErrorObjectOwned>("ok")
            } else {
                // Same shape Solana uses for an unhealthy node, so monitors and
                // load balancers treat a stalled producer as down.
                Err(ErrorObjectOwned::owned(
                    -32005,
                    "Node is unhealthy: block production stalled",
                    None::<()>,
                ))
            }
        })
        .unwrap();

    module
        .register_method("getVersion", |_params, _ctx, _ext| {
            // `solana-core` MUST be a valid semver — the solana CLI parses
            // it and aborts on anything else. Report the Agave SVM base.
            Ok::<_, ErrorObjectOwned>(json!({
                "solana-core": "3.1.14",
                "feature-set": 0,
            }))
        })
        .unwrap();

    module
        .register_method("getSlot", |_params, ctx, _ext| {
            Ok::<_, ErrorObjectOwned>(json!(ctx.current_slot()))
        })
        .unwrap();

    // One block per slot, no skips: block height == slot.
    module
        .register_method("getBlockHeight", |_params, ctx, _ext| {
            Ok::<_, ErrorObjectOwned>(json!(ctx.current_slot()))
        })
        .unwrap();

    module
        .register_method("getEpochInfo", |_params, ctx, _ext| {
            let slot = ctx.current_slot();
            const SLOTS_PER_EPOCH: u64 = 432_000;
            Ok::<_, ErrorObjectOwned>(json!({
                "absoluteSlot": slot,
                "blockHeight": slot,
                "epoch": slot / SLOTS_PER_EPOCH,
                "slotIndex": slot % SLOTS_PER_EPOCH,
                "slotsInEpoch": SLOTS_PER_EPOCH,
                "transactionCount": Value::Null,
            }))
        })
        .unwrap();

    module
        .register_method("getMinimumBalanceForRentExemption", |params, _ctx, _ext| {
            let parsed = params_array(params.parse().map_err(|_| invalid_params("bad params"))?);
            let data_len = parsed.first().and_then(|v| v.as_u64()).unwrap_or(0) as usize;
            let lamports = solana_sdk::rent::Rent::default().minimum_balance(data_len);
            Ok::<_, ErrorObjectOwned>(json!(lamports))
        })
        .unwrap();

    module
        .register_method("getFeeForMessage", |params, ctx, _ext| {
            // Fee = required signatures * lamports_per_signature. Read the
            // signature count from the message header (byte 0, after an
            // optional v0 `0x80` prefix).
            let parsed = params_array(params.parse().map_err(|_| invalid_params("bad params"))?);
            let sigs = parsed
                .first()
                .and_then(|v| v.as_str())
                .and_then(|s| base64::engine::general_purpose::STANDARD.decode(s).ok())
                .map(|bytes| {
                    let off = if bytes.first().is_some_and(|b| b & 0x80 != 0) { 1 } else { 0 };
                    bytes.get(off).copied().unwrap_or(1) as u64
                })
                .unwrap_or(1);
            let fee = sigs.saturating_mul(zul_primitives::constants::LAMPORTS_PER_SIGNATURE);
            Ok::<_, ErrorObjectOwned>(encode::with_context(ctx.current_slot(), json!(fee)))
        })
        .unwrap();

    module
        .register_method("getGenesisHash", |_params, ctx, _ext| {
            let hash = ctx
                .store()
                .genesis_hash()
                .map_err(|e| internal(e.to_string()))?
                .ok_or_else(|| internal("genesis not initialized"))?;
            Ok::<_, ErrorObjectOwned>(json!(bs58::encode(hash).into_string()))
        })
        .unwrap();

    module
        .register_method("getLatestBlockhash", |_params, ctx, _ext| {
            let (slot, hash) = ctx.latest_blockhash();
            Ok::<_, ErrorObjectOwned>(encode::with_context(
                slot,
                json!({
                    "blockhash": bs58::encode(hash).into_string(),
                    "lastValidBlockHeight": slot + zul_primitives::constants::BLOCKHASH_QUEUE_CAPACITY as u64,
                }),
            ))
        })
        .unwrap();

    module
        .register_method("isBlockhashValid", |params, ctx, _ext| {
            let parsed = params_array(params.parse().map_err(|_| invalid_params("bad params"))?);
            let s = parsed
                .first()
                .and_then(|v| v.as_str())
                .ok_or_else(|| invalid_params("expected blockhash string"))?;
            let bytes = bs58::decode(s)
                .into_vec()
                .map_err(|_| invalid_params("invalid blockhash"))?;
            let hash: [u8; 32] = bytes
                .try_into()
                .map_err(|_| invalid_params("blockhash must be 32 bytes"))?;
            Ok::<_, ErrorObjectOwned>(encode::with_context(
                ctx.current_slot(),
                json!(ctx.is_blockhash_valid(&hash)),
            ))
        })
        .unwrap();

    module
        .register_method("getBalance", |params, ctx, _ext| {
            let parsed = params_array(params.parse().map_err(|_| invalid_params("bad params"))?);
            let pubkey = parse_pubkey(parsed.first())?;
            let lamports = ctx
                .store()
                .get_account(&pubkey)
                .map_err(|e| internal(e.to_string()))?
                .map(|a| a.lamports)
                .unwrap_or(0);
            Ok::<_, ErrorObjectOwned>(encode::with_context(ctx.current_slot(), json!(lamports)))
        })
        .unwrap();

    module
        .register_method("getAccountInfo", |params, ctx, _ext| {
            let parsed = params_array(params.parse().map_err(|_| invalid_params("bad params"))?);
            let pubkey = parse_pubkey(parsed.first())?;
            let encoding = config_str(&parsed, "encoding", "base64");
            let value = ctx
                .store()
                .get_account(&pubkey)
                .map_err(|e| internal(e.to_string()))?
                .map(|a| encode::encode_account(&a, &encoding))
                .unwrap_or(Value::Null);
            Ok::<_, ErrorObjectOwned>(encode::with_context(ctx.current_slot(), value))
        })
        .unwrap();

    module
        .register_method("getMultipleAccounts", |params, ctx, _ext| {
            let parsed = params_array(params.parse().map_err(|_| invalid_params("bad params"))?);
            let keys = parsed
                .first()
                .and_then(|v| v.as_array())
                .ok_or_else(|| invalid_params("expected an array of pubkeys"))?;
            let encoding = config_str(&parsed, "encoding", "base64");
            let mut values = Vec::with_capacity(keys.len());
            for key in keys {
                let pubkey = parse_pubkey(Some(key))?;
                let value = ctx
                    .store()
                    .get_account(&pubkey)
                    .map_err(|e| internal(e.to_string()))?
                    .map(|a| encode::encode_account(&a, &encoding))
                    .unwrap_or(Value::Null);
                values.push(value);
            }
            Ok::<_, ErrorObjectOwned>(encode::with_context(ctx.current_slot(), json!(values)))
        })
        .unwrap();

    module
        .register_method("getTokenAccountsByOwner", |params, ctx, _ext| {
            // Minimal: scan store for token accounts owned by the program
            // whose data encodes `owner`. Returns parsed-free raw accounts.
            let parsed = params_array(params.parse().map_err(|_| invalid_params("bad params"))?);
            let owner = parse_pubkey(parsed.first())?;
            let program = parsed
                .get(1)
                .and_then(|c| c.get("programId"))
                .and_then(|v| v.as_str())
                .map(|s| Pubkey::from_str(s))
                .transpose()
                .map_err(|_| invalid_params("invalid programId"))?
                .unwrap_or(spl_token_program_id());
            let encoding = config_str(&parsed, "encoding", "base64");
            let accounts = ctx
                .store()
                .scan_accounts_by_owner(&program, 10_000)
                .map_err(|e| internal(e.to_string()))?;
            // SPL token account layout: mint(32) owner(32) ...
            let mut out = Vec::new();
            for (pubkey, account) in accounts {
                if account.data.len() >= 64 && account.data[32..64] == owner.to_bytes() {
                    out.push(json!({
                        "pubkey": pubkey.to_string(),
                        "account": encode::encode_account(&account, &encoding),
                    }));
                }
            }
            Ok::<_, ErrorObjectOwned>(encode::with_context(ctx.current_slot(), json!(out)))
        })
        .unwrap();

    module
        .register_method("getSignaturesForAddress", |params, ctx, _ext| {
            let parsed = params_array(params.parse().map_err(|_| invalid_params("bad params"))?);
            let address = parse_pubkey(parsed.first())?;
            let config = parsed.get(1);
            let limit = config
                .and_then(|c| c.get("limit"))
                .and_then(|v| v.as_u64())
                .unwrap_or(1000)
                .min(1000) as usize;
            // `before` is a signature: page strictly older than it. Resolve it
            // to its (slot, index) locator and use that as the store cursor.
            let before_sig = parse_opt_signature(config, "before")?;
            let before_cursor = before_sig.and_then(|sig| {
                ctx.store()
                    .get_transaction(&sig)
                    .ok()
                    .flatten()
                    .map(|(loc, _, _)| (loc.slot, loc.index))
            });
            // `until` is a signature: stop once we reach it (exclusive).
            let until_sig = parse_opt_signature(config, "until")?;
            let entries = ctx
                .store()
                .signatures_for_address(&address, before_cursor, limit)
                .map_err(|e| internal(e.to_string()))?;
            let mut out: Vec<Value> = Vec::new();
            for (sig, slot, _) in &entries {
                if Some(*sig) == until_sig {
                    break;
                }
                // Report the tx's execution error (same shape as getTransaction's
                // meta.err) so clients can tell failed txs from successful ones
                // without a getTransaction per signature.
                let err = ctx
                    .store()
                    .get_transaction(sig)
                    .ok()
                    .flatten()
                    .map(|(_, _, meta)| encode::status_to_json(&meta.status))
                    .filter(|v| !v.is_null());
                out.push(encode::signature_entry(sig, *slot, err));
            }
            Ok::<_, ErrorObjectOwned>(json!(out))
        })
        .unwrap();

    module
        .register_method("getProgramAccounts", |params, ctx, _ext| {
            let parsed = params_array(params.parse().map_err(|_| invalid_params("bad params"))?);
            let program = parse_pubkey(parsed.first())?;
            let config = parsed.get(1);
            let encoding = config_str(&parsed, "encoding", "base64");
            let filters = config
                .and_then(|c| c.get("filters"))
                .and_then(|v| v.as_array())
                .cloned()
                .unwrap_or_default();
            let accounts = ctx
                .store()
                .scan_accounts_by_owner(&program, 10_000)
                .map_err(|e| internal(e.to_string()))?;
            let mut out = Vec::new();
            for (pubkey, account) in accounts {
                if !passes_filters(&account.data, &filters) {
                    continue;
                }
                out.push(json!({
                    "pubkey": pubkey.to_string(),
                    "account": encode::encode_account(&account, &encoding),
                }));
            }
            Ok::<_, ErrorObjectOwned>(json!(out))
        })
        .unwrap();

    module
        .register_method("getTransaction", |params, ctx, _ext| {
            let parsed = params_array(params.parse().map_err(|_| invalid_params("bad params"))?);
            let sig = parsed
                .first()
                .and_then(|v| v.as_str())
                .map(Signature::from_str)
                .transpose()
                .map_err(|_| invalid_params("invalid signature"))?
                .ok_or_else(|| invalid_params("expected signature string"))?;
            let found = ctx
                .store()
                .get_transaction(&sig)
                .map_err(|e| internal(e.to_string()))?;
            match found {
                None => Ok::<_, ErrorObjectOwned>(Value::Null),
                Some((locator, tx, meta)) => {
                    let account_keys: Vec<Pubkey> =
                        tx.message.static_account_keys().to_vec();
                    Ok(json!({
                        "slot": locator.slot,
                        "blockTime": Value::Null,
                        "transaction": [
                            base64::engine::general_purpose::STANDARD
                                .encode(bincode::serialize(&tx).unwrap_or_default()),
                            "base64"
                        ],
                        "meta": encode::encode_meta(&meta, &account_keys),
                    }))
                }
            }
        })
        .unwrap();

    module
        .register_method("getSignatureStatuses", |params, ctx, _ext| {
            let parsed = params_array(params.parse().map_err(|_| invalid_params("bad params"))?);
            let sigs = parsed
                .first()
                .and_then(|v| v.as_array())
                .ok_or_else(|| invalid_params("expected array of signatures"))?;
            let mut statuses = Vec::with_capacity(sigs.len());
            for s in sigs {
                let sig = s
                    .as_str()
                    .map(Signature::from_str)
                    .transpose()
                    .map_err(|_| invalid_params("invalid signature"))?;
                let value = match sig {
                    Some(sig) => ctx
                        .store()
                        .get_transaction(&sig)
                        .map_err(|e| internal(e.to_string()))?
                        .map(|(loc, _, meta)| {
                            // `status` is the legacy Result-shaped duplicate of
                            // `err` that the solana CLI still requires.
                            let status = match &meta.status {
                                Ok(()) => json!({ "Ok": Value::Null }),
                                Err(e) => json!({ "Err": format!("{e:?}") }),
                            };
                            json!({
                                "slot": loc.slot,
                                "confirmations": Value::Null,
                                "err": encode::status_to_json(&meta.status),
                                "status": status,
                                "confirmationStatus": "finalized",
                            })
                        })
                        .unwrap_or(Value::Null),
                    None => Value::Null,
                };
                statuses.push(value);
            }
            Ok::<_, ErrorObjectOwned>(encode::with_context(ctx.current_slot(), json!(statuses)))
        })
        .unwrap();

    module
        .register_method("getBlock", |params, ctx, _ext| {
            let parsed = params_array(params.parse().map_err(|_| invalid_params("bad params"))?);
            let slot = parsed
                .first()
                .and_then(|v| v.as_u64())
                .ok_or_else(|| invalid_params("expected slot number"))?;
            let block = ctx
                .store()
                .get_block(slot)
                .map_err(|e| internal(e.to_string()))?;
            match block {
                None => Ok::<_, ErrorObjectOwned>(Value::Null),
                Some(block) => {
                    let signatures: Vec<String> = block
                        .transactions
                        .iter()
                        .filter_map(|t| t.signatures.first().map(|s| s.to_string()))
                        .collect();
                    Ok(json!({
                        "blockhash": bs58::encode(block.header.hash()).into_string(),
                        "previousBlockhash": bs58::encode(block.header.parent_hash).into_string(),
                        "parentSlot": block.header.slot.saturating_sub(1),
                        "blockHeight": block.header.slot,
                        "blockTime": (block.header.timestamp_ms / 1000) as i64,
                        "stateRoot": bs58::encode(block.header.state_root).into_string(),
                        "signatures": signatures,
                    }))
                }
            }
        })
        .unwrap();

    module
        .register_async_method("sendTransaction", |params, ctx, _ext| async move {
            let parsed = params_array(params.parse().map_err(|_| invalid_params("bad params"))?);
            let encoding = config_str(&parsed, "encoding", "base58");
            let tx = decode_transaction(parsed.first(), &encoding)?;
            let sig = ctx
                .submit_transaction(tx)
                .await
                .map_err(|e| invalid_params(e.to_string()))?;
            Ok::<_, ErrorObjectOwned>(json!(sig.to_string()))
        })
        .unwrap();

    module
        .register_async_method("simulateTransaction", |params, ctx, _ext| async move {
            let parsed = params_array(params.parse().map_err(|_| invalid_params("bad params"))?);
            let encoding = config_str(&parsed, "encoding", "base64");
            let tx = decode_transaction(parsed.first(), &encoding)?;
            let result = ctx
                .simulate_transaction(tx)
                .await
                .map_err(|e| invalid_params(e.to_string()))?;
            Ok::<_, ErrorObjectOwned>(encode::with_context(
                ctx.current_slot(),
                json!({
                    "err": result.err,
                    "logs": result.logs,
                    "unitsConsumed": result.units_consumed,
                    "accounts": Value::Null,
                    "returnData": Value::Null,
                }),
            ))
        })
        .unwrap();

    module
        .register_method("getWithdrawalProof", |params, ctx, _ext| {
            let parsed = params_array(params.parse().map_err(|_| invalid_params("bad params"))?);
            let recipient = parse_pubkey(parsed.first())?.to_bytes();
            let num = |v: Option<&Value>| -> Option<u64> {
                v.and_then(|x| x.as_u64().or_else(|| x.as_str().and_then(|s| s.parse().ok())))
            };
            let amount = num(parsed.get(1)).ok_or_else(|| invalid_params("expected amount"))?;
            let nonce = num(parsed.get(2)).ok_or_else(|| invalid_params("expected nonce"))?;
            // Optional 4th param: asset (L1 mint, base58) for SPL withdrawals.
            // Omitted = native ZUL/SOL (all-zero asset id).
            let asset = match parsed.get(3) {
                Some(v) if !v.is_null() => parse_pubkey(Some(v))?.to_bytes(),
                _ => [0u8; 32],
            };
            let (state_root, proof) = ctx
                .withdrawal_proof(&recipient, &asset, amount, nonce)
                .map_err(invalid_params)?;
            Ok::<_, ErrorObjectOwned>(json!({
                "stateRoot": bs58::encode(state_root).into_string(),
                "bitmap": bs58::encode(proof.bitmap).into_string(),
                "siblings": proof
                    .siblings
                    .iter()
                    .map(|s| bs58::encode(s).into_string())
                    .collect::<Vec<_>>(),
            }))
        })
        .unwrap();

    if faucet_enabled {
        module
            .register_async_method("requestAirdrop", |params, ctx, _ext| async move {
                let parsed = params_array(params.parse().map_err(|_| invalid_params("bad params"))?);
                let pubkey = parse_pubkey(parsed.first())?;
                let lamports = parsed
                    .get(1)
                    .and_then(|v| v.as_u64())
                    .ok_or_else(|| invalid_params("expected lamports amount"))?;
                let sig = ctx
                    .request_airdrop(pubkey, lamports)
                    .await
                    .map_err(|e| invalid_params(e.to_string()))?;
                Ok::<_, ErrorObjectOwned>(json!(sig.to_string()))
            })
            .unwrap();
    }

    // ----- WebSocket subscriptions (web3.js confirmTransaction et al.) -----

    module
        .register_subscription(
            "slotSubscribe",
            "slotNotification",
            "slotUnsubscribe",
            |_params, pending, ctx, _ext| async move {
                let Ok(sink) = pending.accept().await else {
                    return;
                };
                let mut rx = ctx.subscribe();
                loop {
                    match rx.recv().await {
                        Ok(n) => {
                            let payload =
                                json!({ "slot": n.slot, "parent": n.parent_slot, "root": n.slot });
                            let Ok(raw) = serde_json::value::to_raw_value(&payload) else {
                                break;
                            };
                            if sink.send(raw).await.is_err() {
                                break;
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            },
        )
        .unwrap();

    module
        .register_subscription(
            "signatureSubscribe",
            "signatureNotification",
            "signatureUnsubscribe",
            |params, pending, ctx, _ext| async move {
                let parsed = params_array(params.parse().unwrap_or(Value::Null));
                let Some(sig) = parsed
                    .first()
                    .and_then(|v| v.as_str())
                    .and_then(|s| Signature::from_str(s).ok())
                else {
                    let _ = pending.reject(invalid_params("invalid signature")).await;
                    return;
                };
                let Ok(sink) = pending.accept().await else {
                    return;
                };
                // Already processed before the client subscribed? Notify now.
                if let Ok(Some((loc, _, meta))) = ctx.store().get_transaction(&sig) {
                    let payload = json!({
                        "context": { "slot": loc.slot },
                        "value": { "err": encode::status_to_json(&meta.status) },
                    });
                    if let Ok(raw) = serde_json::value::to_raw_value(&payload) {
                        let _ = sink.send(raw).await;
                    }
                    return;
                }
                let mut rx = ctx.subscribe();
                loop {
                    match rx.recv().await {
                        Ok(n) => {
                            if let Some((_, err)) = n.signatures.iter().find(|(s, _)| *s == sig) {
                                let payload = json!({
                                    "context": { "slot": n.slot },
                                    "value": { "err": err },
                                });
                                if let Ok(raw) = serde_json::value::to_raw_value(&payload) {
                                    let _ = sink.send(raw).await;
                                }
                                break; // signatureSubscribe fires once
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            },
        )
        .unwrap();

    module
        .register_subscription(
            "accountSubscribe",
            "accountNotification",
            "accountUnsubscribe",
            |params, pending, ctx, _ext| async move {
                let parsed = params_array(params.parse().unwrap_or(Value::Null));
                let Some(pubkey) = parsed
                    .first()
                    .and_then(|v| v.as_str())
                    .and_then(|s| Pubkey::from_str(s).ok())
                else {
                    let _ = pending.reject(invalid_params("invalid pubkey")).await;
                    return;
                };
                let encoding = config_str(&parsed, "encoding", "base64");
                let Ok(sink) = pending.accept().await else {
                    return;
                };
                let mut rx = ctx.subscribe();
                loop {
                    match rx.recv().await {
                        Ok(n) => {
                            if let Some((_, acc)) = n.accounts.iter().find(|(pk, _)| *pk == pubkey) {
                                let value = acc
                                    .as_ref()
                                    .map(|a| encode::encode_account(a, &encoding))
                                    .unwrap_or(Value::Null);
                                let payload =
                                    json!({ "context": { "slot": n.slot }, "value": value });
                                let Ok(raw) = serde_json::value::to_raw_value(&payload) else {
                                    break;
                                };
                                if sink.send(raw).await.is_err() {
                                    break;
                                }
                            }
                        }
                        Err(broadcast::error::RecvError::Lagged(_)) => continue,
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            },
        )
        .unwrap();

    module
}

fn spl_token_program_id() -> Pubkey {
    Pubkey::from_str("TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA").unwrap()
}

/// Concurrent connections served before new ones are refused. Per-IP request
/// rate limiting belongs at the reverse proxy (nginx/Caddy); this caps total
/// resource use so a single client can't exhaust the node.
const MAX_CONNECTIONS: u32 = 1_000;
/// Request bodies above this are rejected. Transactions are <1.5 KB and even
/// large `getMultipleAccounts` batches fit comfortably; the default 10 MB is
/// an unnecessary memory-amplification surface.
const MAX_REQUEST_BODY_BYTES: u32 = 2 * 1024 * 1024;
/// Responses can be large (`getBlock`, `getTokenAccountsByOwner`), so keep this
/// generous but bounded.
const MAX_RESPONSE_BODY_BYTES: u32 = 16 * 1024 * 1024;

/// Running RPC servers: HTTP on the main port, WebSocket on the conventional
/// `port + 1` (web3.js derives the WS endpoint that way). Both speak the full
/// HTTP+WS protocol; the split just matches client defaults.
pub struct ServeHandles {
    pub http_addr: SocketAddr,
    pub ws_addr: SocketAddr,
    pub http: ServerHandle,
    pub ws: ServerHandle,
}

async fn start_server<B: RpcBackend>(
    addr: SocketAddr,
    backend: Arc<B>,
    faucet_enabled: bool,
) -> Result<(SocketAddr, ServerHandle), std::io::Error> {
    let config = ServerConfig::builder()
        .max_connections(MAX_CONNECTIONS)
        .max_request_body_size(MAX_REQUEST_BODY_BYTES)
        .max_response_body_size(MAX_RESPONSE_BODY_BYTES)
        .build();
    let server = Server::builder().set_config(config).build(addr).await?;
    let local_addr = server.local_addr()?;
    let handle = server.start(build_module(backend, faucet_enabled));
    Ok((local_addr, handle))
}

/// Start the JSON-RPC servers (HTTP + WebSocket). `faucet_enabled` gates
/// `requestAirdrop`. Returns handles that keep them running until stopped.
pub async fn serve<B: RpcBackend>(
    http_addr: SocketAddr,
    ws_addr: SocketAddr,
    backend: Arc<B>,
    faucet_enabled: bool,
) -> Result<ServeHandles, std::io::Error> {
    let (http_addr, http) = start_server(http_addr, backend.clone(), faucet_enabled).await?;
    let (ws_addr, ws) = start_server(ws_addr, backend, faucet_enabled).await?;
    Ok(ServeHandles {
        http_addr,
        ws_addr,
        http,
        ws,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn params_array_normalizes_shapes() {
        assert_eq!(params_array(json!([1, 2])).len(), 2);
        assert_eq!(params_array(Value::Null).len(), 0);
        assert_eq!(params_array(json!("x")).len(), 1);
    }

    #[test]
    fn config_str_reads_nested_encoding() {
        let params = vec![json!("addr"), json!({ "encoding": "base58" })];
        assert_eq!(config_str(&params, "encoding", "base64"), "base58");
        assert_eq!(config_str(&params, "missing", "default"), "default");
    }

    #[test]
    fn decode_transaction_rejects_garbage() {
        assert!(decode_transaction(Some(&json!("!!!notbase64!!!")), "base64").is_err());
        assert!(decode_transaction(None, "base64").is_err());
    }

    #[test]
    fn decode_transaction_accepts_both_encodings() {
        use solana_sdk::{
            hash::Hash, pubkey::Pubkey, signature::Keypair, signer::Signer,
            transaction::Transaction,
        };
        use solana_system_interface::instruction as system_instruction;

        let payer = Keypair::new();
        let ix = system_instruction::transfer(&payer.pubkey(), &Pubkey::new_unique(), 1);
        let tx = VersionedTransaction::from(Transaction::new_signed_with_payer(
            &[ix],
            Some(&payer.pubkey()),
            &[&payer],
            Hash::new_from_array([1u8; 32]),
        ));
        let bytes = bincode::serialize(&tx).unwrap();
        let b64 = base64::engine::general_purpose::STANDARD.encode(&bytes);
        let b58 = bs58::encode(&bytes).into_string();

        // A web3.js client (base64) and a CLI client (base58) both decode,
        // regardless of the stated hint.
        assert_eq!(
            decode_transaction(Some(&json!(b64)), "base64").unwrap().signatures,
            tx.signatures
        );
        assert_eq!(
            decode_transaction(Some(&json!(b58)), "base58").unwrap().signatures,
            tx.signatures
        );
        // Wrong hint still works thanks to the fallback.
        assert_eq!(
            decode_transaction(Some(&json!(b64)), "base58").unwrap().signatures,
            tx.signatures
        );
    }

    #[test]
    fn parse_pubkey_roundtrip() {
        let pk = Pubkey::new_unique();
        let parsed = parse_pubkey(Some(&json!(pk.to_string()))).unwrap();
        assert_eq!(parsed, pk);
        assert!(parse_pubkey(Some(&json!("nope"))).is_err());
    }

    #[test]
    fn program_account_filters() {
        let data = vec![1u8, 2, 3, 4, 5, 6, 7, 8];
        // dataSize: exact length only.
        assert!(passes_filters(&data, &[json!({ "dataSize": 8 })]));
        assert!(!passes_filters(&data, &[json!({ "dataSize": 7 })]));
        // memcmp: bytes (base58) must equal the window at offset.
        let at2 = bs58::encode([3u8, 4]).into_string();
        assert!(passes_filters(&data, &[json!({ "memcmp": { "offset": 2, "bytes": at2 } })]));
        let wrong = bs58::encode([9u8, 9]).into_string();
        assert!(!passes_filters(&data, &[json!({ "memcmp": { "offset": 2, "bytes": wrong } })]));
        // Out-of-range memcmp fails closed; multiple filters AND together.
        let one = bs58::encode([1u8]).into_string();
        assert!(!passes_filters(&data, &[json!({ "memcmp": { "offset": 100, "bytes": one } })]));
        assert!(passes_filters(
            &data,
            &[json!({ "dataSize": 8 }), json!({ "memcmp": { "offset": 0, "bytes": bs58::encode([1u8]).into_string() } })]
        ));
    }
}
