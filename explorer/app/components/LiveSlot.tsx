"use client";

import { useEffect, useState } from "react";

/// Polls /api/slot and shows the live chain height, so the overview feels
/// alive even between full page loads.
export function LiveSlot({ initial }: { initial: number }) {
  const [slot, setSlot] = useState(initial);

  useEffect(() => {
    let alive = true;
    const tick = async () => {
      try {
        const res = await fetch("/api/slot", { cache: "no-store" });
        if (!res.ok) return;
        const data = (await res.json()) as { slot: number };
        if (alive && typeof data.slot === "number") setSlot(data.slot);
      } catch {
        /* node unreachable; keep last value */
      }
    };
    const id = setInterval(tick, 2000);
    return () => {
      alive = false;
      clearInterval(id);
    };
  }, []);

  return (
    <span className="value live mono">{slot.toLocaleString()}</span>
  );
}
