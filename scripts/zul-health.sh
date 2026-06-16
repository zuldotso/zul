#!/usr/bin/env bash
# Zul mainnet health check — run by zul-health.timer every few minutes. Logs to
# the journal (journalctl -u zul-health) and, if a webhook URL is present in
# config/mainnet/alert-webhook.txt (gitignored), POSTs an alert there on any
# problem. No webhook = log-only. Payload is Discord-style {"content": ...};
# for Slack change the key to "text".
set -u

REPO="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
WEBHOOK_FILE="$REPO/config/mainnet/alert-webhook.txt"
DEPLOYER="${ZUL_BRIDGE_AUTHORITY:-ETCUTxbKrCTQST1eWAEKc9gPtsiNQhQ9BAHwxwjQMDbZ}"
SOL_MIN="${ZUL_SOL_MIN:-0.5}"
L1_RPC="${ZUL_L1_PUBLIC_RPC:-https://api.mainnet-beta.solana.com}"
HEIGHT_STATE="/tmp/zul-health-height"
export PATH="$HOME/.local/share/solana/install/active_release/bin:$PATH"

problems=()
rpc() { curl -s --max-time 8 http://127.0.0.1:8899 -H 'content-type: application/json' -d "{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"$1\"}"; }

# Node up + healthy.
systemctl is-active --quiet zul-node@mainnet || problems+=("zul-node@mainnet service not active")
rpc getHealth | grep -q '"result":"ok"' || problems+=("node getHealth != ok")

# Public endpoint (Caddy + TLS) up.
systemctl is-active --quiet caddy || problems+=("caddy service not active")
curl -s --max-time 10 https://rpc-mainnet.zul.so -H 'content-type: application/json' \
  -d '{"jsonrpc":"2.0","id":1,"method":"getHealth"}' | grep -q '"result":"ok"' \
  || problems+=("public RPC https://rpc-mainnet.zul.so failed")

# L2 block height advancing (not stalled).
h=$(rpc getBlockHeight | grep -o '"result":[0-9]*' | cut -d: -f2)
if [ -f "$HEIGHT_STATE" ] && [ -n "$h" ]; then
  prev=$(cat "$HEIGHT_STATE" 2>/dev/null)
  [ -n "$prev" ] && [ "$h" -le "$prev" ] && problems+=("L2 block height stalled at $h")
fi
[ -n "$h" ] && echo "$h" > "$HEIGHT_STATE"

# Bridge authority funded (settlement pauses near zero).
bal=$(solana balance "$DEPLOYER" --url "$L1_RPC" 2>/dev/null | awk '{print $1}')
if [ -n "$bal" ]; then
  awk -v b="$bal" -v m="$SOL_MIN" 'BEGIN{exit !(b<m)}' \
    && problems+=("bridge authority SOL low: $bal < $SOL_MIN — settlement will pause near 0")
else
  problems+=("could not read bridge authority SOL balance")
fi

ts=$(date -u +%Y-%m-%dT%H:%M:%SZ)
if [ "${#problems[@]}" -eq 0 ]; then
  echo "[$ts] OK — node+caddy up, height=$h, bridge SOL=$bal"
  exit 0
fi

msg="[ALERT] Zul mainnet ($ts):
$(printf -- '- %s\n' "${problems[@]}")"
echo "$msg"
if [ -s "$WEBHOOK_FILE" ]; then
  url=$(head -n1 "$WEBHOOK_FILE" | tr -d '[:space:]')
  payload=$(python3 -c 'import json,sys; print(json.dumps({"content": sys.stdin.read()}))' <<<"$msg")
  curl -s --max-time 10 -H 'content-type: application/json' -d "$payload" "$url" >/dev/null \
    || echo "(webhook post failed)"
fi
exit 0
