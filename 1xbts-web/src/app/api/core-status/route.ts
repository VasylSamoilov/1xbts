import { waitForMscReady } from "@/lib/grpc/client";
import { waitForHlrReady } from "@/lib/grpc/hlr-client";
import { waitForSmscReady } from "@/lib/grpc/smsc-client";

export const dynamic = "force-dynamic";

export async function GET() {
  const checks = await Promise.allSettled([waitForMscReady(), waitForHlrReady(), waitForSmscReady()]);
  const components = {
    msc: checks[0].status === "fulfilled",
    hlr: checks[1].status === "fulfilled",
    smsc: checks[2].status === "fulfilled",
  };
  const available = Object.values(components).filter(Boolean).length;
  return Response.json({
    state: available === 3 ? "Operational" : available > 0 ? "Degraded" : "Unavailable",
    components,
  });
}
