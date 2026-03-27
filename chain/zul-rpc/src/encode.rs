//! Solana-compatible JSON shaping: response envelopes, account encoding,
//! transaction encoding. These pure functions are the part most worth
//! testing — wallets break on subtle shape mismatches.

use base64::Engine;
use zul_primitives::meta::ExecutedTransactionMeta;
use zul_store::StoredAccount;
use serde_json::{json, Value};
use solana_sdk::pubkey::Pubkey;

/// `{ context: { apiVersion, slot }, value }` envelope used by most methods.
/// `apiVersion` MUST be a valid semver — the solana CLI parses it from the
/// response context (e.g. on `getBalance`) and aborts otherwise.
pub fn with_context(slot: u64, value: Value) -> Value {
    json!({
        "context": { "apiVersion": "3.1.14", "slot": slot },
        "value": value,
    })
}

/// Encode account bytes under the requested encoding. Defaults to base64
/// (web3.js default for raw data). base58 is supported for small accounts.
pub fn encode_account_data(data: &[u8], encoding: &str) -> Value {
    match encoding {
        "base58" => json!(bs58::encode(data).into_string()),
        // "base64" and anything else fall back to base64.
        _ => json!([
            base64::engine::general_purpose::STANDARD.encode(data),
            "base64"
        ]),
    }
}

/// Shape an account for `getAccountInfo` / `getMultipleAccounts`.
pub fn encode_account(account: &StoredAccount, encoding: &str) -> Value {
    json!({
        "lamports": account.lamports,
        "owner": account.owner.to_string(),
        "data": encode_account_data(&account.data, encoding),
        "executable": account.executable,
        "rentEpoch": account.rent_epoch,
        "space": account.data.len(),
    })
}

/// A `getSignaturesForAddress` entry.
pub fn signature_entry(sig: &solana_sdk::signature::Signature, slot: u64, err: Option<Value>) -> Value {
    json!({
        "signature": sig.to_string(),
        "slot": slot,
        "err": err,
        "memo": Value::Null,
        "blockTime": Value::Null,
        "confirmationStatus": "finalized",
    })
}

/// Encode `ExecutedTransactionMeta` as the `meta` object of `getTransaction`.
pub fn encode_meta(meta: &ExecutedTransactionMeta, account_keys: &[Pubkey]) -> Value {
    json!({
        "err": status_to_json(&meta.status),
        "fee": meta.fee,
        "preBalances": meta.pre_balances,
        "postBalances": meta.post_balances,
        "innerInstructions": [],
        "logMessages": meta.log_messages,
        "preTokenBalances": [],
        "postTokenBalances": [],
        "rewards": [],
        "computeUnitsConsumed": meta.compute_units_consumed,
        "loadedAddresses": { "writable": [], "readonly": [] },
        "accountKeys": account_keys.iter().map(|k| k.to_string()).collect::<Vec<_>>(),
    })
}

/// Solana renders a successful status as JSON `null`, errors as an object.
pub fn status_to_json(status: &Result<(), solana_sdk::transaction::TransactionError>) -> Value {
    match status {
        Ok(()) => Value::Null,
        Err(e) => json!({ "err": format!("{e:?}") }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample_account() -> StoredAccount {
        StoredAccount {
            lamports: 1_000_000,
            data: vec![1, 2, 3, 4],
            owner: Pubkey::new_unique(),
            executable: false,
            rent_epoch: u64::MAX,
        }
    }

    #[test]
    fn context_envelope_shape() {
        let v = with_context(42, json!({ "lamports": 5 }));
        assert_eq!(v["context"]["slot"], 42);
        assert_eq!(v["value"]["lamports"], 5);
        assert!(v["context"]["apiVersion"].is_string());
    }

    #[test]
    fn account_base64_is_default_and_decodes() {
        let acc = sample_account();
        let v = encode_account(&acc, "base64");
        let arr = v["data"].as_array().unwrap();
        assert_eq!(arr[1], "base64");
        let decoded = base64::engine::general_purpose::STANDARD
            .decode(arr[0].as_str().unwrap())
            .unwrap();
        assert_eq!(decoded, acc.data);
        assert_eq!(v["space"], 4);
        assert_eq!(v["executable"], false);
    }

    #[test]
    fn account_base58_encoding() {
        let acc = sample_account();
        let v = encode_account(&acc, "base58");
        let s = v["data"].as_str().unwrap();
        assert_eq!(bs58::decode(s).into_vec().unwrap(), acc.data);
    }

    #[test]
    fn ok_status_is_null_err_is_object() {
        assert_eq!(status_to_json(&Ok(())), Value::Null);
        let err = status_to_json(&Err(
            solana_sdk::transaction::TransactionError::AccountNotFound,
        ));
        assert!(err.is_object());
    }
}
