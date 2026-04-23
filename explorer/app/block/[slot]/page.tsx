import { notFound } from "next/navigation";
import { getBlock } from "@/lib/queries";
import { shorten, timeAgo } from "@/lib/format";
import { TxList } from "@/app/components/rows";

export const dynamic = "force-dynamic";

export default async function BlockPage({
  params,
}: {
  params: Promise<{ slot: string }>;
}) {
  const { slot } = await params;
  const slotNum = Number(slot);
  if (!Number.isFinite(slotNum)) notFound();

  const block = await getBlock(slotNum).catch(() => null);
  if (!block) notFound();

  return (
    <>
      <div className="page-head">
        <div>
          <div className="eyebrow">Block</div>
          <h1 className="mono" style={{ color: "var(--violet)" }}>#{block.slot}</h1>
        </div>
        <div style={{ marginLeft: "auto", display: "flex", gap: 10 }}>
          {block.slot > 0 && (
            <a className="badge neutral" href={`/block/${block.slot - 1}`}>← prev</a>
          )}
          <a className="badge neutral" href={`/block/${block.slot + 1}`}>next →</a>
        </div>
      </div>

      <div className="kv" style={{ marginBottom: 26 }}>
        <div className="k">Blockhash</div>
        <div className="v mono">{block.blockhash}</div>
        <div className="k">Parent hash</div>
        <div className="v mono">
          {block.slot > 0 ? (
            <a className="hash" href={`/block/${block.slot - 1}`}>{block.parentHash}</a>
          ) : (
            <span className="hash dim">{block.parentHash} (genesis)</span>
          )}
        </div>
        <div className="k">State root</div>
        <div className="v mono">{block.stateRoot || "—"}</div>
        <div className="k">Transactions</div>
        <div className="v">{block.txCount}</div>
        <div className="k">Timestamp</div>
        <div className="v">
          {new Date(block.timestampMs).toISOString()}{" "}
          <span className="cell-dim">({timeAgo(block.timestampMs)})</span>
        </div>
      </div>

      <div className="card">
        <div className="section-title card-pad" style={{ paddingBottom: 0 }}>
          <h2>Transactions in this block</h2>
        </div>
        <TxList txs={block.transactions} />
      </div>
    </>
  );
}
