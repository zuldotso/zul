export declare const FIELD_MODULUS = 21888242871839275222246405745257275088548364400416034343698204186575808495617n;
export type FieldBytes = Uint8Array;
export declare function frToBytes(value: bigint): FieldBytes;
export declare function frFromBytes(bytes: FieldBytes): bigint;
export declare function frToBytesBE(value: bigint): Uint8Array;
export declare function randomFr(): bigint;
