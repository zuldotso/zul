import { describe, it, expect } from "vitest";
import { BincodeReader, BincodeWriter } from "../src/bincode.js";
import {
  Asset,
  encodeShield,
  encodeTransfer,
  encodeUnshield,
  decodeInstructionTag,
} from "../src/instruction.js";

describe("bincode primitives", () => {
  it("encodes u32/u64 little-endian", () => {
    const bytes = new BincodeWriter().u32(1).u64(258n).finish();
    // variant 1 (LE) then 258 = 0x0102 (LE)
    expect(Array.from(bytes.slice(0, 4))).toEqual([1, 0, 0, 0]);
    expect(Array.from(bytes.slice(4, 12))).toEqual([2, 1, 0, 0, 0, 0, 0, 0]);
  });

  it("vecU8 prefixes a u64 length", () => {
    const bytes = new BincodeWriter().vecU8(new Uint8Array([9, 8, 7])).finish();
    expect(Array.from(bytes.slice(0, 8))).toEqual([3, 0, 0, 0, 0, 0, 0, 0]);
    expect(Array.from(bytes.slice(8))).toEqual([9, 8, 7]);
  });

  it("round-trips through the reader", () => {
    const bytes = new BincodeWriter()
      .u32(2)
      .u64(1000n)
      .fixedBytes(new Uint8Array(32).fill(7), 32)
      .vecU8(new Uint8Array([1, 2]))
      .finish();
    const r = new BincodeReader(bytes);
    expect(r.u32()).toBe(2);
    expect(r.u64()).toBe(1000n);
    expect(r.fixedBytes(32)[0]).toBe(7);
    expect(Array.from(r.vecU8())).toEqual([1, 2]);
    expect(r.done()).toBe(true);
  });
});

describe("PoolInstruction encoding", () => {
  const secret = new Uint8Array(32).fill(3);
  const root = new Uint8Array(32).fill(1);

  it("shield: native asset layout", () => {
    const data = encodeShield({
      asset: Asset.native(),
      amount: 1000n,
      secret,
      encryptedNote: new Uint8Array([0xab, 0xcd]),
    });
    expect(decodeInstructionTag(data)).toBe(0);
    // variant(4) + asset_tag(4) + amount(8) + secret(32) + vec_len(8) + 2
    expect(data.length).toBe(4 + 4 + 8 + 32 + 8 + 2);
  });

  it("shield: spl asset includes the 32-byte mint", () => {
    const mint = new Uint8Array(32).fill(9);
    const data = encodeShield({
      asset: Asset.spl(mint),
      amount: 5n,
      secret,
      encryptedNote: new Uint8Array(0),
    });
    // asset tag 1 then mint
    expect(Array.from(data.slice(4, 8))).toEqual([1, 0, 0, 0]);
    expect(Array.from(data.slice(8, 40))).toEqual(Array(32).fill(9));
  });

  it("transfer: fixed arrays carry no inner length", () => {
    const data = encodeTransfer({
      proof: new Uint8Array(256).fill(2),
      root,
      nullifiers: [new Uint8Array(32).fill(4), new Uint8Array(32).fill(5)],
      commitments: [new Uint8Array(32).fill(6), new Uint8Array(32).fill(7)],
      fee: 5000n,
      encryptedNotes: [new Uint8Array([1]), new Uint8Array([2])],
    });
    expect(decodeInstructionTag(data)).toBe(1);
    // variant(4) + proof(8+256) + root(32) + nf(64) + cm(64) + fee(8)
    //   + 2 * (vec_len 8 + 1)
    expect(data.length).toBe(4 + 8 + 256 + 32 + 64 + 64 + 8 + 2 * (8 + 1));
  });

  it("unshield: layout length", () => {
    const data = encodeUnshield({
      proof: new Uint8Array(256).fill(2),
      root,
      nullifier: new Uint8Array(32).fill(4),
      changeCommitment: new Uint8Array(32).fill(5),
      asset: Asset.native(),
      amount: 900n,
      recipient: new Uint8Array(32).fill(8),
      encryptedChangeNote: new Uint8Array([1, 2, 3]),
    });
    expect(decodeInstructionTag(data)).toBe(2);
    // variant(4)+proof(8+256)+root(32)+nf(32)+changecm(32)+asset(4)
    //   +amount(8)+recipient(32)+vec(8+3)
    expect(data.length).toBe(4 + 8 + 256 + 32 + 32 + 32 + 4 + 8 + 32 + 8 + 3);
  });
});
