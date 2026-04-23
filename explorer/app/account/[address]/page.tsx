import { getAccountActivity } from "@/lib/queries";
import { TxList } from "@/app/components/rows";

export const dynamic = "force-dynamic";

export default async function AccountPage({
  params,
}: {
  params: Promise<{ address: string }>;
}) {
  const { address } = await params;
  const decoded = decodeURIComponent(address);
  const txs = await getAccountActivity(decoded).catch(() => []);

  return (
    <>
      <div className="page-head">
        <div style={{ minWidth: 0 }}>
          <div className="eyebrow">Account</div>
          <h1 className="mono" style={{ fontSize: 20, wordBreak: "break-all" }}>{decoded}</h1>
        </div>
      </div>

      <div className="card">
        <div className="section-title card-pad" style={{ paddingBottom: 0 }}>
          <h2>Transaction history</h2>
          <span className="cell-dim">{txs.length} recent</span>
        </div>
        <TxList txs={txs} />
      </div>
    </>
  );
}
