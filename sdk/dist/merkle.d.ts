export declare const TREE_DEPTH = 26;
export declare const EMPTY_LEAF = 770212417220141184n;
export declare class MerkleTree {
    private zeros;
    private layers;
    private ready;
    constructor();
    private init;
    append(leaf: bigint): Promise<number>;
    root(): Promise<bigint>;
    path(leafIndex: number): Promise<bigint[]>;
}
