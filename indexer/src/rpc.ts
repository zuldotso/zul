// Tiny JSON-RPC client for the ZUL node. Uses fetch against the
// Solana-compatible endpoint; tolerant of our minimal response shapes.

export interface RpcBlock {
  blockhash: string;
  previousBlockhash: string;
  parentSlot: number;
  blockHeight: number;
  blockTime: number;
  stateRoot: string;
  signatures: string[];
}

export interface RpcTransaction {
  slot: number;
  meta: {
    err: unknown;
    fee: number;
    computeUnitsConsumed: number;
    logMessages: string[];
    accountKeys: string[];
  } | null;
  transaction: [string, string]; // [base64, "base64"]
}

export class RpcClient {
  private id = 0;

  constructor(private endpoint: string) {}

  private async call<T>(method: string, params: unknown[]): Promise<T> {
    const res = await fetch(this.endpoint, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: ++this.id, method, params }),
    });
    if (!res.ok) throw new Error(`rpc ${method} http ${res.status}`);
    const body = (await res.json()) as { result?: T; error?: { message: string } };
    if (body.error) throw new Error(`rpc ${method}: ${body.error.message}`);
    return body.result as T;
  }

  getSlot(): Promise<number> {
    return this.call<number>("getSlot", []);
  }

  getGenesisHash(): Promise<string> {
    return this.call<string>("getGenesisHash", []);
  }

  getBlock(slot: number): Promise<RpcBlock | null> {
    return this.call<RpcBlock | null>("getBlock", [slot]);
  }

  getTransaction(signature: string): Promise<RpcTransaction | null> {
    return this.call<RpcTransaction | null>("getTransaction", [
      signature,
      { encoding: "base64", maxSupportedTransactionVersion: 0 },
    ]);
  }
}
