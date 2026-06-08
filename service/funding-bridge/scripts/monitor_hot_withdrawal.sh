#!/bin/bash

set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_DIR="$(dirname "$SCRIPT_DIR")"
WORKSPACE_DIR="$(cd "$SCRIPT_DIR/../../.." && pwd)"

if [ -f "$PROJECT_DIR/.env" ]; then
  set -a
  source "$PROJECT_DIR/.env"
  set +a
fi

NONCE="${1:-1776177129000001087571}"
RECEIVER="${2:-${STELLAR_SECRET_KEY:+$(stellar keys address templar-hot-mainnet 2>/dev/null || true)}}"
INTERVAL="${3:-15}"
ONCE="${ONCE:-0}"
ENCODER_WORKSPACE="${HOT_ENCODER_WORKSPACE:-$WORKSPACE_DIR}"

if [ -z "$RECEIVER" ]; then
  echo "receiver is required as arg 2 or via local Stellar identity" >&2
  exit 1
fi

if ! ENCODED_RECEIVER="$(python3 - <<'PY' "$RECEIVER" "$ENCODER_WORKSPACE")"; then
import sys
addr = sys.argv[1]
root_arg = sys.argv[2]
src = None
bin_path = None
try:
    import subprocess, tempfile, pathlib, os
    root = pathlib.Path(root_arg).resolve()
    deps = root / 'target' / 'debug' / 'deps'
    if not deps.is_dir():
        raise RuntimeError(f'missing dependency directory: {deps}')
    bs58 = next((root / 'target' / 'debug' / 'deps').glob('libbs58-*.rlib'))
    stellar_xdr = next((root / 'target' / 'debug' / 'deps').glob('libstellar_xdr-*.rlib'))
    src = tempfile.NamedTemporaryFile('w', suffix='.rs', delete=False)
    src.write('use std::str::FromStr; use stellar_xdr::curr::{Limited, Limits, ScAddress, ScVal, WriteXdr}; fn main(){ let addr=std::env::args().nth(1).unwrap(); let sc=ScAddress::from_str(&addr).unwrap(); let val=ScVal::Address(sc); let mut bytes=Vec::new(); let mut w=Limited::new(&mut bytes, Limits::none()); val.write_xdr(&mut w).unwrap(); println!("{}", bs58::encode(bytes).into_string()); }')
    src.close()
    bin_path = src.name + '.bin'
    subprocess.check_call([
        'rustc', src.name, '-o', bin_path,
        '--extern', f'bs58={bs58}',
        '--extern', f'stellar_xdr={stellar_xdr}',
        '-L', f'dependency={root / "target" / "debug" / "deps"}',
        '--edition=2021'
    ], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    out = subprocess.check_output([bin_path, addr], text=True).strip()
    print(out)
except Exception as error:
    print(f'failed to derive ENCODED_RECEIVER: {error}', file=sys.stderr)
    sys.exit(1)
finally:
    import os
    if src is not None:
        try:
            os.unlink(src.name)
        except OSError:
            pass
    if bin_path is not None:
        try:
            os.unlink(bin_path)
        except OSError:
            pass
PY
)"; then
  echo "Failed to derive ENCODED_RECEIVER; set HOT_ENCODER_WORKSPACE or fix local encoder deps." >&2
  exit 1
fi

if [ -z "$ENCODED_RECEIVER" ]; then
  echo "Failed to derive ENCODED_RECEIVER; encoder returned an empty value." >&2
  exit 1
fi

while true; do
  echo "===== $(date -u '+%Y-%m-%dT%H:%M:%SZ') ====="
  echo "nonce: $NONCE"
  echo "receiver: $RECEIVER"
  echo "encoded_receiver: $ENCODED_RECEIVER"

  echo "-- stellar balance --"
  python3 - <<'PY' "$RECEIVER"
import sys, json, urllib.request
addr=sys.argv[1]
url=f'https://horizon.stellar.org/accounts/{addr}'
with urllib.request.urlopen(url, timeout=30) as r:
    data=json.load(r)
for b in data.get('balances', []):
    if b.get('asset_type') == 'native':
        print(b.get('balance'))
PY

  echo "-- hot withdraw/sign --"
  node - <<'JS' "$NONCE"
const nonce = process.argv[2];
for (const url of ['https://rpc1.hotdao.ai/withdraw/sign','https://rpc2.hotdao.ai/withdraw/sign']) {
  try {
    const res = await fetch(url, {method:'POST', headers:{'Content-Type':'application/json'}, body: JSON.stringify({nonce})});
    console.log(url, res.status, await res.text());
  } catch (e) {
    console.log(url, 'ERR', String(e));
  }
}
JS

  echo "-- hot clear_completed_withdrawal --"
  node - <<'JS' "$NONCE"
const nonce = process.argv[2];
const headers = {'omni-version':'v2','Content-Type':'application/json','Referer':'https://near-intents.org','Origin':'https://near-intents.org'};
for (const url of ['https://api0.herewallet.app/api/v1/transactions/clear_completed_withdrawal','https://api2.herewallet.app/api/v1/transactions/clear_completed_withdrawal']) {
  try {
    const res = await fetch(url, {method:'POST', headers, body: JSON.stringify({nonce})});
    console.log(url, res.status, await res.text());
  } catch (e) {
    console.log(url, 'ERR', String(e));
  }
}
JS

  echo "-- near pending withdrawals --"
  python3 - <<'PY' "$ENCODED_RECEIVER"
import sys, json, urllib.request, base64
encoded = sys.argv[1]
args = {'receiver_id': encoded, 'chain_id': 1100}
payload={"jsonrpc":"2.0","id":"dontcare","method":"query","params":{"request_type":"call_function","finality":"final","account_id":"v2_1.omni.hot.tg","method_name":"get_withdrawals_by_receiver","args_base64":base64.b64encode(json.dumps(args).encode()).decode()}}
req=urllib.request.Request('https://rpc.mainnet.near.org', data=json.dumps(payload).encode(), headers={'Content-Type':'application/json'})
with urllib.request.urlopen(req, timeout=60) as r:
    print(r.read().decode())
PY

  if [ "$ONCE" = "1" ]; then
    break
  fi

  sleep "$INTERVAL"
done
