// Live chain height. Proxies getSlot to the node RPC so the browser never
// needs the node endpoint directly (and CORS stays simple).

import { NextResponse } from "next/server";

export const dynamic = "force-dynamic";

const RPC_URL = process.env.ZUL_RPC_URL ?? "http://127.0.0.1:8899";

export async function GET() {
  try {
    const res = await fetch(RPC_URL, {
      method: "POST",
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ jsonrpc: "2.0", id: 1, method: "getSlot", params: [] }),
      cache: "no-store",
    });
    const body = (await res.json()) as { result?: number };
    return NextResponse.json({ slot: body.result ?? 0 });
  } catch {
    return NextResponse.json({ slot: null }, { status: 200 });
  }
}
