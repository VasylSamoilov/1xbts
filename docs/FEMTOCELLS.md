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

The Plus profile uses the same `airvana_airave2` Fx2/media implementation as
`airave-25-2x`. It configures the IWF user and band/channel pair atomically,
uses individual stages for the remaining parameters, and retains GPS reference
and timezone configuration. It omits legacy per-PDSN table/counter status
queries that the Plus firmware rejects with CWMP `9005 Invalid Parametername`.

An in-place comparison with the unmodified `airave-25-2x` profile and the netmask
present confirmed an installed IPsec tunnel, SIP registration, timing lock, and
BC1/1175 on air. A pilot PN change from 0 to 4 and back to 0 also applied
successfully through the stock provisioning stages. However, stock status
collection failed on paths including `OneXpdsnAdminStatus` and
`OneXpdsnHealthStatus` under `OneXPcfPdsnTableEntry.1`. Removing those queries
allows status collection to complete; it does not change the PDSN implementation
or establish that packet-data service works. The comparison was not a
factory-reset provisioning test. Handset voice/SMS and packet-data/EV-DO
operation were not verified in these tests.

### Service radio versus 800 MHz beacons

**Normal 1x service on this tested 2.5+ is BC1 / 1900 MHz PCS.** Its BC0/BC10
capability belongs to a separate beacon subsystem. The beacon transmits a real
CDMA downlink containing discovery/redirection signaling, but it is not a
complete two-way service carrier with traffic capacity for calls, SMS, or data.
There is no supported configuration found in this investigation that moves
normal service to 800 MHz.

The tested unit is HubBub-C1-600-RT, hardware `750721.00.03`, FCC ID
`QHYHUBBUBC4501-RT`. The manufacturer's Curtis-Straus report EN1043-2,
page 4, explicitly identifies five transmitters:

| Transmitter | Frequency range | Role |
| --- | --- | --- |
| One-X and EVDO | 1930-1990 MHz | Service downlink |
| Beacon BC1 | 1930-1990 MHz | PCS beacon |
| Beacon BC0 | 869-894 MHz | Cellular beacon |
| Beacon BC10 | 862-869 MHz | 800 MHz beacon |

See the [manufacturer's FCC test report](https://fccid.io/QHYHUBBUBC4501-RT/Test-Report/Test-Report-pdf-2015587.pdf).
Those are transmitter ranges; they do not establish an 800 MHz uplink receiver
or two-way service capability. The normal `OneXSectorMO` controls rejected
BC0/384, BC0/777, BC0/1175, and BC10/476, including paired band/channel updates
in both setter orders. BC1/725 and BC1/1175 were accepted.

A beacon lets a compatible phone discover the femtocell on a familiar
acquisition channel and redirects it to the service frequency. In this setup,
the intended path is BC0/777 discovery followed by BC1/1175 service. Beacon RF
transmission was verified; the actual redirection messages and handset behavior
were not. A BC0-only phone cannot follow a redirect to BC1. A phone that supports
both bands still depends on its PRL and network-selection rules. See the
[3GPP2 femtocell architecture document](https://www.3g4g.co.uk/SmallCells/Femtocells_WP_Architecture_1107_3GPP2.pdf),
section 5.2, for beacon discovery and redirection.

### Beacon experiment results

The separate `OneXBeaconMO` subsystem accepted BC0/777, centered at
893.310 MHz. An SDR confirmed repeating wideband bursts at a requested 0 dBm
and their disappearance when the beacon was disabled. A subsequent requested
10 dBm setting was accepted and read back. The supplied `airave-25plus` profile
only discovers/reads beacon status; it does not configure or enable a beacon.
The experiment was configured through an ignored local overlay and the beacon
was subsequently disabled at the operator's request.

The following firmware requirements were established during the experiment:

- Initial creation of a beacon frequency-table row requires all eleven fields:
  index, band, channel, PN offset, duration, offset, power, SID, NID, PZID, and
  transmission format. Table instances must be discovered per device.
- Despite an error asking for "DO or OneX", `transmissionFormat` expects the
  strings `"0"` for DO or `"1"` for OneX.
- Configure the row while the beacon is disabled. Unlock its ATA beacon control
  before enabling the overall beacon; reversing that order faults.
- The default 80 ms interval is rejected by the scheduler as too short. A
  32-slot duration in a 64-slot cycle worked: 2.56 seconds in each 5.12-second
  cycle. Duration equal to the cycle fails because the scheduler needs a gap.
- Software configuration acceptance alone is not proof of RF output. Supported
  console status showed the modem sector open and beacon radio UP during active
  intervals; the SDR provided independent confirmation. No RF was observed in
  the initial -30 dBm tests, which did not establish a hardware power floor.

The internal OneXApplication console is available on device loopback port
56001. After `enable`, supported read commands include `show beacon
configuration`, `show beacon status`, and `show radio config all`. An
experimental `xml-get object OneXBeaconMO name 0` command crashed the radio
application, requiring its process supervisor to recover it; avoid that query.
No firmware was patched or flashed.

Changing the advertised band does not change standard channel-frequency
mapping. BC0/777 is 893.310 MHz downlink / 848.310 MHz uplink, whereas BC10/777
is 936.425 / 897.425 MHz. Relabeling the service carrier as BC10 cannot produce
BC0/777 service. The 893.310 MHz beacon result must not be described as a
working 800 MHz traffic carrier.

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
service settings. The BTS summary list used by the web interface is also
captured at core startup in this version. After adding/removing sites or changing
their displayed radio metadata, restart `femto-core` to refresh that list; a
successful provisioning reload alone does not refresh it.

To apply provisioning immediately after reload, request a CWMP sync for one FEID
once it has an authenticated management path:

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
