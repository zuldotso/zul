import { getRecentBlocks } from "@/lib/queries";
import { BlockList } from "@/app/components/rows";

export const dynamic = "force-dynamic";

export default async function BlocksPage() {
  const blocks = await getRecentBlocks(50).catch(() => []);
  return (
    <>
      <div className="page-head">
        <div>
          <div className="eyebrow">Chain</div>
          <h1>Blocks</h1>
        </div>
      </div>
      <div className="card">
        <BlockList blocks={blocks} />
      </div>
    </>
  );
}
