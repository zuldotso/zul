// Off-chain mirror of the pool's incremental Merkle tree, used to build the
// path witnesses snarkjs needs. Matches chain/zul-privacy/src/state.rs
// (depth 26, Poseidon2 nodes, fixed empty-leaf constant).

import { treeNode } from "./poseidon.js";

export const TREE_DEPTH = 26;
// Must equal zero_hashes()[0] in state.rs.
export const EMPTY_LEAF = 770212417220141184n;

export class MerkleTree {
  private zeros: bigint[] = [];
  private layers: bigint[][] = [];
  private ready: Promise<void>;

  constructor() {
    this.ready = this.init();
  }

  private async init(): Promise<void> {
    this.zeros = [EMPTY_LEAF];
    for (let i = 1; i <= TREE_DEPTH; i++) {
      this.zeros[i] = await treeNode(this.zeros[i - 1], this.zeros[i - 1]);
    }
    this.layers = Array.from({ length: TREE_DEPTH + 1 }, () => []);
  }

  /// Append leaves in index order (the same order the chain inserts them).
  async append(leaf: bigint): Promise<number> {
    await this.ready;
    const index = this.layers[0].length;
    this.layers[0].push(leaf);
    let i = index;
    for (let level = 0; level < TREE_DEPTH; level++) {
      const layer = this.layers[level];
      const isRight = i % 2 === 1;
      const left = isRight ? layer[i - 1] : layer[i];
      const right = isRight ? layer[i] : this.zeros[level];
      const parent = await treeNode(left, right);
      i = Math.floor(i / 2);
      if (this.layers[level + 1].length <= i) {
        this.layers[level + 1][i] = parent;
      } else {
        this.layers[level + 1][i] = parent;
      }
    }
    return index;
  }

  async root(): Promise<bigint> {
    await this.ready;
    if (this.layers[0].length === 0) return this.zeros[TREE_DEPTH];
    return this.layers[TREE_DEPTH][0];
  }

  /// Sibling path for `leafIndex`, leaf level first (depth entries).
  async path(leafIndex: number): Promise<bigint[]> {
    await this.ready;
    const elements: bigint[] = [];
    let i = leafIndex;
    for (let level = 0; level < TREE_DEPTH; level++) {
      const layer = this.layers[level];
      const siblingIndex = i % 2 === 0 ? i + 1 : i - 1;
      elements.push(
        siblingIndex < layer.length ? layer[siblingIndex] : this.zeros[level],
      );
      i = Math.floor(i / 2);
    }
    return elements;
  }
}
