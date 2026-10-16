import { getFemtoManagementClient } from "@/lib/grpc/femto-client";

export const dynamic = "force-dynamic";

export async function GET(request: Request) {
  const url = new URL(request.url);
  const baseStation = url.searchParams.get("baseStation");
  const peerId = url.searchParams.get("peerId");
  if (!baseStation || !peerId) {
    return Response.json({ error: "baseStation and peerId are required" }, { status: 400 });
  }
  try {
    const status = await getFemtoManagementClient(baseStation).getFemtoCellStatus({ peerId });
    return Response.json(status);
  } catch (error) {
    const message = error instanceof Error ? error.message : "unknown error";
    return Response.json({ error: message }, { status: 502 });
  }
}
