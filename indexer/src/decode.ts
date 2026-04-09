// Best-effort decoding of a base64 transaction to surface the programs it
// invokes and the fee payer, without a full Solana parser. We read the
// legacy/v0 message header, the static account keys, and each instruction's
// program-id index.

const B58 = "123456789ABCDEFGHJKLMNPQRSTUVWXYZabcdefghijkmnopqrstuvwxyz";

function base58Encode(bytes: Uint8Array): string {
  let zeros = 0;
  while (zeros < bytes.length && bytes[zeros] === 0) zeros++;
  const digits: number[] = [];
  for (let i = zeros; i < bytes.length; i++) {
    let carry = bytes[i];
    for (let j = 0; j < digits.length; j++) {
      carry += digits[j] << 8;
      digits[j] = carry % 58;
      carry = (carry / 58) | 0;
    }
    while (carry > 0) {
      digits.push(carry % 58);
      carry = (carry / 58) | 0;
    }
  }
  let out = "1".repeat(zeros);
  for (let i = digits.length - 1; i >= 0; i--) out += B58[digits[i]];
  return out;
}

function base64Decode(s: string): Uint8Array {
  if (typeof Buffer !== "undefined") return new Uint8Array(Buffer.from(s, "base64"));
  const binary = atob(s);
  const out = new Uint8Array(binary.length);
  for (let i = 0; i < binary.length; i++) out[i] = binary.charCodeAt(i);
  return out;
}

// The enshrined pool program id (POOL_PROGRAM_ID in zul-privacy), base58.
function poolProgramIdBase58(): string {
  const bytes = new Uint8Array(32);
  bytes[0] = 0x05;
  bytes[1] = 0x21;
  return base58Encode(bytes);
}

const POOL_PROGRAM_ID_B58 = poolProgramIdBase58();

const KNOWN_PROGRAMS: Record<string, string> = {
  "11111111111111111111111111111111": "system",
  TokenkegQfeZyiNwAJbNbGKPFXCWuBvf9Ss623VQ5DA: "spl-token",
  TokenzQdBNbLqP5VEhdkAS6EPFLC1PHnBqCXEpPxuEb: "spl-token-2022",
  ATokenGPvbdGVxr1b2hvZbsiqW5xWH25efTNsLJA8knL: "associated-token-account",
  MemoSq4gqABAXKb96qnH8TysNcWxMyWCqXgDLGmfcHr: "memo",
  ComputeBudget111111111111111111111111111111: "compute-budget",
  [POOL_PROGRAM_ID_B58]: "shielded-pool",
};

export interface DecodedTx {
  feePayer: string;
  accountKeys: string[];
  programs: string[];
  touchedPool: boolean;
}

export function decodeTransaction(base64: string): DecodedTx | null {
  try {
    const bytes = base64Decode(base64);
    // A real signed transaction is at least: 1 sig (65) + header (3) +
    // 1 key (33) + blockhash (32) + 1 instruction. Reject anything too
    // small to be a transaction before trusting the parse.
    if (bytes.length < 1 + 64 + 3 + 1 + 32 + 32) return null;
    let offset = 0;

    // Signatures: compact-u16 count, then 64 bytes each.
    const [sigCount, sigLenBytes] = readCompactU16(bytes, offset);
    if (sigCount === 0) return null; // every real tx is signed
    offset += sigLenBytes + sigCount * 64;

    // Versioned prefix: high bit set means v0.
    if (bytes[offset] & 0x80) {
      offset += 1;
    }

    // Message header: 3 bytes.
    offset += 3;

    // Static account keys.
    const [keyCount, keyLenBytes] = readCompactU16(bytes, offset);
    offset += keyLenBytes;
    if (keyCount === 0) return null; // at least the fee payer
    const accountKeys: string[] = [];
    for (let i = 0; i < keyCount; i++) {
      if (offset + 32 > bytes.length) return null;
      accountKeys.push(base58Encode(bytes.slice(offset, offset + 32)));
      offset += 32;
    }

    // Recent blockhash.
    offset += 32;
    if (offset > bytes.length) return null;

    // Instructions.
    const [ixCount, ixLenBytes] = readCompactU16(bytes, offset);
    offset += ixLenBytes;
    const programIndexes = new Set<number>();
    for (let i = 0; i < ixCount; i++) {
      if (offset >= bytes.length) return null;
      const programIdIndex = bytes[offset];
      offset += 1;
      programIndexes.add(programIdIndex);
      const [accLen, accLenBytes] = readCompactU16(bytes, offset);
      offset += accLenBytes + accLen;
      const [dataLen, dataLenBytes] = readCompactU16(bytes, offset);
      offset += dataLenBytes + dataLen;
      if (offset > bytes.length) return null;
    }

    const programIds = [...programIndexes]
      .map((idx) => accountKeys[idx])
      .filter((k): k is string => Boolean(k));
    const programs = programIds.map((id) => KNOWN_PROGRAMS[id] ?? id);
    return {
      feePayer: accountKeys[0],
      accountKeys,
      programs,
      touchedPool: programIds.includes(POOL_PROGRAM_ID_B58),
    };
  } catch {
    return null;
  }
}

function readCompactU16(bytes: Uint8Array, offset: number): [number, number] {
  let value = 0;
  let len = 0;
  for (;;) {
    // compact-u16 is at most 3 bytes; running past the buffer or that limit
    // means the input is not a real transaction.
    if (offset + len >= bytes.length || len >= 3) {
      throw new Error("compact-u16 out of range");
    }
    const byte = bytes[offset + len];
    value |= (byte & 0x7f) << (7 * len);
    len += 1;
    if ((byte & 0x80) === 0) break;
  }
  return [value, len];
}

export { base58Encode, POOL_PROGRAM_ID_B58 };
