import { getFemtoManagementClient } from "@/lib/grpc/femto-client";
import { FemtoRadioAdministrativeState } from "@/lib/proto/femto_management/v1/service";

export async function POST(request: Request) {
  const body = await request.json() as {
    baseStation?: string;
    peerId?: string;
    action?: "enable" | "disable" | "sync" | "reboot";
  };
  if (!body.baseStation || !body.peerId || !body.action) {
    return Response.json({ error: "baseStation, peerId and action are required" }, { status: 400 });
  }
  const client = getFemtoManagementClient(body.baseStation);
  const cell = { peerId: body.peerId };
  try {
    const result = body.action === "sync"
      ? await client.requestProvisioningSync(cell)
      : body.action === "reboot"
        ? await client.rebootFemtocell(cell)
        : await client.setFemtoRadioAdministrativeState({
            cell,
            state: body.action === "enable"
              ? FemtoRadioAdministrativeState.FEMTO_RADIO_ADMINISTRATIVE_STATE_ENABLED
              : FemtoRadioAdministrativeState.FEMTO_RADIO_ADMINISTRATIVE_STATE_DISABLED,
          });
    return Response.json(result);
  } catch (error) {
    const message = error instanceof Error ? error.message : "unknown error";
    return Response.json({ error: message }, { status: 502 });
  }
}
