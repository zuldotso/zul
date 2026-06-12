export declare class BincodeWriter {
    private chunks;
    u32(value: number): this;
    u64(value: bigint): this;
    fixedBytes(bytes: Uint8Array, len: number): this;
    vecU8(bytes: Uint8Array): this;
    finish(): Uint8Array;
}
export declare class BincodeReader {
    private bytes;
    private view;
    private offset;
    constructor(bytes: Uint8Array);
    u32(): number;
    u64(): bigint;
    fixedBytes(len: number): Uint8Array;
    vecU8(): Uint8Array;
    done(): boolean;
}
