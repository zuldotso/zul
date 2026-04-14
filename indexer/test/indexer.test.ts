import { describe, it, expect } from "vitest";
import { Indexer } from "../src/indexer.js";
import type { RpcBlock, RpcTransaction } from "../src/rpc.js";
import { base58Encode } from "../src/decode.js";

// A fake RPC with a scripted chain.
class FakeRpc {
  constructor(
    private slot: number,
    private blocks: Map<number, RpcBlock>,
    private txs: Map<string, RpcTransaction>,
  ) {}
  async getSlot() {
    return this.slot;
  }
  async getGenesisHash() {
    return "GENESIS";
  }
  async getBlock(slot: number) {
    return this.blocks.get(slot) ?? null;
  }
  async getTransaction(sig: string) {
    return this.txs.get(sig) ?? null;
  }
}

// A fake Prisma capturing the writes the indexer makes.
function fakePrisma() {
  const blocks = new Map<string, any>();
  const transactions = new Map<string, any>();
  const addressSignatures = new Set<string>();
  let sync: { id: number; lastSlot: bigint; genesisHash: string } | null = null;
  return {
    store: { blocks, transactions, addressSignatures, get sync() { return sync; } },
    block: {
      upsert: async ({ where, create }: any) => {
        blocks.set(where.slot.toString(), create ?? {});
      },
    },
    transaction: {
      upsert: async ({ where, create }: any) => {
        transactions.set(where.signature, create ?? {});
      },
    },
    addressSignature: {
      create: async ({ data }: any) => {
        const key = `${data.address}:${data.signature}`;
        if (addressSignatures.has(key)) throw new Error("unique violation");
        addressSignatures.add(key);
      },
    },
    syncState: {
      // Real Prisma applies `create` only when the row is absent.
      upsert: async ({ create, update }: any) => {
        sync = sync ? { ...sync, ...update } : { ...create };
        return sync;
      },
      update: async ({ data }: any) => {
        sync = { ...sync!, ...data };
        return sync;
      },
    },
  };
}

function tx(sig: string, keys: string[]): RpcTransaction {
  return {
    slot: 1,
    meta: {
      err: null,
      fee: 5000,
      computeUnitsConsumed: 150,
      logMessages: ["Program log: ok"],
      accountKeys: keys,
    },
    transaction: ["", "base64"], // decode falls back to meta.accountKeys
  };
}

describe("indexer tick", () => {
  it("indexes new blocks and transactions, advances the cursor", async () => {
    const payer = base58Encode(new Uint8Array(32).fill(1));
    const blocks = new Map<number, RpcBlock>([
      [
        1,
        {
          blockhash: "B1",
          previousBlockhash: "G",
          parentSlot: 0,
          blockHeight: 1,
          blockTime: 1000,
          stateRoot: "SR1",
          signatures: ["sig1"],
        },
      ],
    ]);
    const txs = new Map<string, RpcTransaction>([["sig1", tx("sig1", [payer])]]);
    const rpc = new FakeRpc(1, blocks, txs) as any;
    const prisma = fakePrisma() as any;

    const indexer = new Indexer({ prisma, rpc });
    const indexed = await indexer.tick();

    expect(indexed).toBe(1);
    expect(prisma.store.blocks.get("1").blockhash).toBe("B1");
    expect(prisma.store.blocks.get("1").stateRoot).toBe("SR1");
    expect(prisma.store.transactions.get("sig1").feePayer).toBe(payer);
    expect(prisma.store.sync.lastSlot).toBe(1n);

    // Idempotent: re-running with no new head indexes nothing.
    expect(await indexer.tick()).toBe(0);
  });

  it("skips empty slots without failing", async () => {
    const rpc = new FakeRpc(3, new Map(), new Map()) as any;
    const prisma = fakePrisma() as any;
    const indexer = new Indexer({ prisma, rpc });
    const indexed = await indexer.tick();
    expect(indexed).toBe(3); // slots counted
    expect(prisma.store.blocks.size).toBe(0); // but nothing stored
    expect(prisma.store.sync.lastSlot).toBe(3n);
  });
});
