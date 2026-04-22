import { shorten, formatZul, timeAgo } from "@/lib/format";
import type { BlockRow, TxRow } from "@/lib/queries";

export function StatusBadge({ success }: { success: boolean }) {
  return (
    <span className={`badge ${success ? "ok" : "fail"}`}>
      {success ? "success" : "failed"}
    </span>
  );
}

export function BlockList({ blocks }: { blocks: BlockRow[] }) {
  if (blocks.length === 0) {
    return <div className="empty">No blocks indexed yet. Start the node and indexer.</div>;
  }
  return (
    <div className="rows">
      {blocks.map((b) => (
        <a key={b.slot} href={`/block/${b.slot}`} className="row row-blocks">
          <span className="cell-slot mono">#{b.slot}</span>
          <span className="hash dim">{shorten(b.blockhash, 10, 8)}</span>
          <span className="cell-dim">{b.txCount} tx</span>
          <span className="cell-dim hide-sm">{timeAgo(b.timestampMs)}</span>
        </a>
      ))}
    </div>
  );
}

export function TxList({ txs }: { txs: TxRow[] }) {
  if (txs.length === 0) {
    return <div className="empty">No transactions yet.</div>;
  }
  return (
    <div className="rows">
      {txs.map((t) => (
        <a key={t.signature} href={`/tx/${t.signature}`} className="row row-txs">
          <span className="hash">
            {shorten(t.signature, 10, 8)}
            {t.touchedPool && <span className="badge pool" style={{ marginLeft: 10 }}>shielded</span>}
          </span>
          <span className="cell-dim hide-sm mono">#{t.slot}</span>
          <span className="cell-dim">{formatZul(t.fee)} ZUL</span>
          <span><StatusBadge success={t.success} /></span>
        </a>
      ))}
    </div>
  );
}
