import { describe, it, expect } from "vitest";
import { MerkleTree } from "../src/merkle.js";
import { frToBytes } from "../src/field.js";

// Pinned to chain/zul-privacy/tests/tree_root_emit.rs. The SDK's tree mirror
// must produce byte-identical roots to the chain's incremental tree, or
// proofs built here reference paths the node rejects.
const EMPTY_ROOT =
  "755ece909e258a8989e9d803927048844745460d568389cffc6f34b7cd800a16";
const ROOT_123 =
  "9f17becb707814be908f2d692fc58c5fee0b54d06a97da98d4d94d07bf82470e";

const hex = (u8: Uint8Array) => Buffer.from(u8).toString("hex");

describe("merkle tree mirrors the chain", () => {
  it("empty root matches the chain constant", async () => {
    const tree = new MerkleTree();
    expect(hex(frToBytes(await tree.root()))).toBe(EMPTY_ROOT);
  });

  it("three-leaf root matches the chain constant", async () => {
    const tree = new MerkleTree();
    for (const n of [1n, 2n, 3n]) await tree.append(n);
    expect(hex(frToBytes(await tree.root()))).toBe(ROOT_123);
  });
});
