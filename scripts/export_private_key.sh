#!/usr/bin/env bash
set -e

KEYPAIR_FILE="${1:-/home/tiktoor/.config/solana/id.json}"

if [ ! -f "$KEYPAIR_FILE" ]; then
    echo "❌ Keypair file not found at: $KEYPAIR_FILE"
    exit 1
fi

python3 -c "
import json, sys

ALPHABET = '123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz'
with open('$KEYPAIR_FILE') as f:
    data = bytes(json.load(f))

num = int.from_bytes(data, 'big')
res = ''
while num > 0:
    num, rem = divmod(num, 58)
    res = ALPHABET[rem] + res

pad = len(data) - len(data.lstrip(b'\x00'))
b58_key = '1' * pad + res

print('========================================================================')
print('           SOLANA WALLET PRIVATE KEY (BASE58 FORMAT)')
print('========================================================================')
print('Public Address : FL1UpMfPybHWgbM5GwzT8VhGseMMARj5guEnuoGunhZG')
print('Private Key    : ' + b58_key)
print('========================================================================')
print('⚠️  SECURITY WARNING: Never share this private key with anyone.')
print('📱 TO IMPORT INTO PHANTOM / SOLFLARE:')
print('   1. Open Phantom or Solflare on your mobile device.')
print('   2. Go to Settings -> Manage Accounts -> Add / Connect Wallet.')
print('   3. Select \"Import Private Key\".')
print('   4. Paste the Base58 string shown above and click Import.')
print('========================================================================')
"
