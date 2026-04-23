import { getRecentTransactions } from "@/lib/queries";
import { TxList } from "@/app/components/rows";

export const dynamic = "force-dynamic";

export default async function TxsPage() {
  const txs = await getRecentTransactions(50).catch(() => []);
  return (
    <>
      <div className="page-head">
        <div>
          <div className="eyebrow">Chain</div>
          <h1>Transactions</h1>
        </div>
      </div>
      <div className="card">
        <TxList txs={txs} />
      </div>
    </>
  );
}
