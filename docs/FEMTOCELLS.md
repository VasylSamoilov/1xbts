# Femtocell configuration and reloads

Both services merge `config/femto.json` with `config/femto.local.json`.
Objects merge recursively; arrays replace the base array. Keep credentials in
the ignored local file. Each site must have a distinct FEID and cell identity.

## Airave 2.5+

Use `profile: "airave-25plus"` for the Airvana `cdmaFAP3` tested with firmware
`6.5.01.010`, hardware `750721.00.03`. Set the explicit ACS username from the
device; this unit uses `0005B9-cdmaFAP3-<FEID>`, whereas the default username
builder uses `cdmaFAP`.

With the default virtual-IP pool, add this top-level block to the local overlay:

```json
{
  "segw": {
    "vip_netmask": "255.255.255.0"
  }
}
```

Use a netmask appropriate for a different VIP network. This is an explicit IKE
configuration attribute, separate from the address-pool allocation CIDR. The
Plus authenticated successfully without it, then immediately sent an ESP
DELETE with zero data transferred. Returning the netmask kept the CHILD_SA
installed. Existing OG Airave tunnels also worked with the attribute present.
The generator leaves the attribute absent unless configured.

The Plus profile keeps the Airvana Fx2/media implementation, configures the IWF
user and band/channel pair atomically, and uses individual stages for the
remaining configuration parameters to make faults clear. It omits the OG firmware's per-PDSN table and counter status paths,
which this firmware rejects with CWMP 9005. GPS reference and timezone
configuration remain enabled. Test the configured band and channel: the tested
unit rejected BC0/384, BC0/777, BC0/1175, and BC10/476 with 9002. Paired
band/channel updates in both orders also failed. BC1/725 and BC1/1175 were
accepted. These results establish the behavior of this firmware interface;
they do not establish the RF front-end capabilities. No separate RF-frequency
versus advertised-band control was found in the inspected CWMP objects.
This profile has been exercised for 1x provisioning; EV-DO/data are not verified.

BC0-only handset support does not imply BC10 support. BC0/777 maps to
893.310 MHz downlink / 848.310 MHz uplink; BC10/777 maps to 936.425 /
897.425 MHz. BC0/1175 is in the extended BC0 subclass (864.570 / 819.570
MHz). BC1/1175 is 1988.750 / 1908.750 MHz. A BC10 band label cannot produce
BC0/777 frequencies under the standard channel mapping. See 3GPP2 C.S0057
sections 2.1.1 and 2.1.11. Establishing alternate 800 MHz operation on this unit
requires further firmware/RF hardware investigation.

The supplied label identifies HubBub-C1-600-RT, FCC ID
`QHYHUBBUBC4501-RT`. Its FCC grant lists 862.9â€“867.9 MHz,
870.03â€“889.2 MHz, and 1931.25â€“1988.75 MHz transmit ranges:
https://fccid.io/QHYHUBBUBC4501-RT
The live firmware has a separate OneXBeaconMO subsystem, with the beacon
currently disabled and zero beacon intervals. The lower-band chains might
serve beacon operation rather than the service sector; this is a hypothesis,
not a confirmed hardware limitation. The grant alone does not establish
usable 800 MHz two-way service through OneXSectorMO. BC0/777 (893.310 MHz)
is outside the listed cellular transmit range in this filing.

## Applying edits

The SeGW configuration controller polls both config files about once a second.
It renders `config/femto-segw/swanctl/swanctl.conf`, and the SeGW entrypoint
loads changed output with `swanctl --load-all`. Ordinary site/credential/pool
edits do not require restarting either SeGW container. Updated settings apply
to new negotiations; existing tunnels can retain their negotiated settings.
Do not edit the generated swanctl file as a persistent fix.

The femto core reads config at startup and on the `ReloadConfig` RPC. It does
not automatically reload when the file changes. From the repository root,
with `grpcurl` installed on the host:

```sh
grpcurl -plaintext -import-path proto \
  -proto femto_management/v1/service.proto -d '{}' \
  127.0.0.1:17024 femto_management.v1.FemtoManagementService/ReloadConfig
```

This reloads sites, profiles, CWMP credentials, SIP users, A12 PSK, cell state,
and served cells. An invalid config returns an error before the active state
is replaced. It does not rebind listener sockets or change startup-only
service settings. To apply provisioning immediately after reload, request a
CWMP sync for one FEID once it has an authenticated management path:

```sh
grpcurl -plaintext -import-path proto \
  -proto femto_management/v1/service.proto -d '{"peer_id":"YOUR_FEID"}' \
  127.0.0.1:17024 femto_management.v1.FemtoManagementService/RequestProvisioningSync
```

The endpoint does not expose server reflection, so supply the proto as shown.
Without grpcurl, the straightforward alternative for loading the file is:

```sh
docker compose --profile femto restart femto-core
```

This briefly interrupts ACS/SIP/FCS; it does not restart the IPsec gateway.
MSC, HLR, PDSN, postgres, and fou-nat do not need restarting solely because
`femto.local.json` changed. Any separate edits to those services' own configs
must be handled independently.

Listener addresses, SIP/ACS realms, background-sync timing, and other
startup-only core settings require a femto-core restart. Compose environment
or network changes require recreation. If recreating femto-segw, also recreate
femto-core because it shares that container's network namespace, and refresh
the management proxy. This interrupts all femtocell tunnels:

```sh
docker compose --profile femto up -d --force-recreate \
  femto-segw femto-core femto-management-proxy
```

For Rust source changes, rebuild the image before recreating the affected
service. The optional netmask field is implemented in the configuration
controller; updating only its image can preserve current IPsec and SIP
sessions:

```sh
docker compose --profile femto build femto-segw-config
docker compose --profile femto up -d --no-deps femto-segw-config
```

## Checking state

```sh
docker exec 1xbts-femto-segw swanctl --list-sas
docker logs --tail 100 1xbts-femto-segw-config
docker logs --tail 100 1xbts-femto-core
```

Look for an installed CHILD_SA with data counters, successful ACS status
readback, SIP registration, GPS/timing lock, and `OneXSectorOperState=0`
(on air). An established IKE_SA alone does not establish a usable data tunnel.
