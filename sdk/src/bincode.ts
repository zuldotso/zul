// Minimal bincode (v1, default config: little-endian, fixint, u32 enum
// variants) encoder/decoder — just enough to mirror the Rust
// `PoolInstruction` wire format in chain/zul-privacy/src/instruction.rs.

export class BincodeWriter {
  private chunks: Uint8Array[] = [];

  u32(value: number): this {
    const b = new Uint8Array(4);
    new DataView(b.buffer).setUint32(0, value >>> 0, true);
    this.chunks.push(b);
    return this;
  }

  u64(value: bigint): this {
    const b = new Uint8Array(8);
    new DataView(b.buffer).setBigUint64(0, BigInt(value), true);
    this.chunks.push(b);
    return this;
  }

  /// Fixed-size byte array (no length prefix).
  fixedBytes(bytes: Uint8Array, len: number): this {
    if (bytes.length !== len) {
      throw new Error(`expected ${len} bytes, got ${bytes.length}`);
    }
    this.chunks.push(bytes.slice());
    return this;
  }

  /// Vec<u8>: u64 length prefix + bytes.
  vecU8(bytes: Uint8Array): this {
    this.u64(BigInt(bytes.length));
    this.chunks.push(bytes.slice());
    return this;
  }

  finish(): Uint8Array {
    let total = 0;
    for (const c of this.chunks) total += c.length;
    const out = new Uint8Array(total);
    let offset = 0;
    for (const c of this.chunks) {
      out.set(c, offset);
      offset += c.length;
    }
    return out;
  }
}

export class BincodeReader {
  private view: DataView;
  private offset = 0;

  constructor(private bytes: Uint8Array) {
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  }

  u32(): number {
    const v = this.view.getUint32(this.offset, true);
    this.offset += 4;
    return v;
  }

  u64(): bigint {
    const v = this.view.getBigUint64(this.offset, true);
    this.offset += 8;
    return v;
  }

  fixedBytes(len: number): Uint8Array {
    const out = this.bytes.slice(this.offset, this.offset + len);
    this.offset += len;
    return out;
  }

  vecU8(): Uint8Array {
    const len = Number(this.u64());
    return this.fixedBytes(len);
  }

  done(): boolean {
    return this.offset === this.bytes.length;
  }
}
