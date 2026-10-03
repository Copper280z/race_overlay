# MyChron6 2T Wi-Fi link protocol

Protocol description derived from `mychron_dump.pcapng` (kept locally, not
committed): a 185-second capture of RaceCapture3 (Windows laptop) talking to
an AiM MyChron6 2T over the logger's built-in Wi-Fi hotspot, covering live
data display, the
saved-datalog list, and one file download.

Everything below is derived from the capture and, where marked, from a live
re-verification against the same device. Items not yet pinned down are listed
in [Open questions](#open-questions). Field names in `backticks` are
descriptive labels invented for this document, not official AiM names.

## Capture provenance

| File | `mychron_dump.pcapng`, 2 431 packets, single Ethernet-link interface |
| Timestamps | nanosecond resolution (`if_tsresol`); 2026-09-29 23:02:24.9 – 23:05:29.9 UTC |
| Device (Wi-Fi AP) | `11.0.0.1`, MAC `80:f3:da:xx:xx:xx` (MyChron6 2T) |
| Client | `11.0.0.2`, MAC `f4:d1:08:xx:xx:xx` (Windows laptop running RaceCapture3) |
| TCP session 1 | client port 63615 → device :2000, t≈123.4–150 s (live data, lists) |
| TCP session 2 | client port 63620 → device :2000, t≈172.5–180.4 s (file download, list refresh) |
| Wi-Fi | SSID of the form `AiM-MYC6-<serial>`, open — no WPA; device serves DHCP + DNS on `11.0.0.0/24` |
| Live verification | 2026-09-29, from a second test computer over a USB Wi-Fi dongle; artifacts retained on that host under `~/mychron_probe/` |

The device also answers DNS (UDP 53) for the laptop; all DNS/mDNS traffic in
the capture is Windows background noise (`time.windows.com`,
`mobile.events.data.microsoft.com`, …) and carries no protocol meaning.

## Transport map

| Port | Proto | Direction observed | Purpose |
| --- | --- | --- | --- |
| 2000 | TCP | client → device (device listens) | Command/response protocol ("STCP" framing below) |
| 36002 | UDP | client → device, device → client | 1.1 s keepalive + device descriptor |
| 53 | UDP | client → device | Plain DNS resolver on the device (noise here) |

## UDP 36002 keepalive / discovery

For the whole capture the client sends the 6-byte ASCII payload `aim-ka` to
`11.0.0.1:36002` every ~1.1 s (121 packets). The device answers each one with
a fixed-size **236-byte descriptor** (120 replies):

```
ec000000                                          u32le datagram length, including this word (0xec = 236)
02000000 0b000001 00000600 ...                    counters/capability fields (mostly zeros)
00000000...0c2e0200                               (offset 0x54) 'idn' record, see below
... 01000000 ...                                  tail, mostly zeros
```

Two bytes drift over time: `0x0E` went 6→7 and `0x11` went 0→2 during the
capture, and after a full power cycle the device still answers 7/2 —
persisted state, not an uptime counter; meaning unknown. The `idn` blob at
offset 84 is byte-identical across power cycles and matches the TCP info
blob's `MST` record except two build-ish bytes (`a8 01` vs `0e 02`):

```
69 64 6e "idn" | 01 | 38 00 | a8 01 | b5 01 00 00 | 61 3c 16 02 | 02 2e 0c 00 | ...
```

## TCP 2000 message framing ("STCP")

Both directions use an ASCII-delimited frame with a payload checksum:

```
'<' 'h' 'S' 'T' <tag:2> <payload_len:u32le> 0x00 '>'
<payload, payload_len bytes>
'<' 'S' 'T' <tag:2> <sum16:u16le> '>'
```

* `tag` is `CP` or `NC` (observed values). The device sends **only `CP`**
  frames. The client uses `NC` for object operations and `CP` for the hello,
  time writes, acknowledgements, and download reads.
* `sum16` = sum of all payload bytes mod 0x10000, little-endian.
  Example: hello payload `00000000 06080000` sums to 0x000e → trailer
  `<STCP 0e 00 3e>`. Verified on every message in both sessions.
* Total frame overhead is 12 + 8 = 20 bytes (the trailer is `<`, `ST`, the
  tag, the sum, `>`).

Concrete first exchange of both sessions:

```
client: 3c6853544350 08000000 00 3e  00000000 06080000  3c53544350 0e00 3e
        "<hSTCP"    len=8            payload          "<STCP" sum  ">"

device: "<hSTCP" len=8 "00000000 06090000" "<STCP" 0f00 ">"      (06 08 -> 06 09)
```

## Transaction model

The client drives everything. An operation is a 64-byte `NC` frame:

```
NC request payload (64 bytes)
offset  size  observed meaning
0       4     0
4       4     0
8       2     object/command id (u16le)
10      2     kind (u16le)
12      4     flags (0x40000000 seen on time-sync requests, else 0)
16      4     0 (request); response header: length of the data message body
20      4     0 (request); device echoes 0x0000ffc0 = 65472 ("max chunk")
24      4     1 (request); device status code, see below
28      4     0
32      4     stream-stop requests write 0xffffffff here
32      32    NUL-padded device path for file operations, overlying the
              field above (e.g. "1:/mem/a_0089.xrz", "0:/lgo/splash.bmp")
```

For each `NC` the device answers, in order:

1. **Echo `CP-64`**: the request payload with `len@16` cleared, the 65472
   chunk value at offset 20, and status `0x0A09` at offset 24 (a plain ack).
2. **Response-header `CP-64`** (when data will follow or an error occurs):
   same shape, `len@16` = length of the body inside the data message,
   status `0x0A11` (data follows) or `0x0A1D` (no data / empty result).
   During file open this field instead carries the **file size**.
3. **Data message** `CP` with a 4-byte leading status word (always
   `00000000` = OK observed) followed by the body — pushed only after the
   client acks the response header with a zeroed `CP-4`. Without that ack
   the data never arrives (verified live).
4. The capture also shows the client acking received data messages with a
   zeroed `CP-4`; the live tests show this second ack (and the `ACK0`
   after a time write) is optional. Caution: a zeroed `CP-4` is
   indistinguishable from a read-at-offset-0 request, so avoid it inside
   file-transfer exchanges.

Observed statuses: request=1, `0x0A09` ack, `0x0A11` data-ready, `0x0A1D`
no-data; k=1 echoes carried `0x0A01` in the capture but `0x0A09` in the
live re-verification (possibly "clock not yet synced").

### Observed operations

| NC id | kind | Purpose (evidence) |
| --- | --- | --- |
| — (CP-8) | — | Hello `06 08 0000`, device replies `06 09 0000` |
| 0x10 | 1 | Get device info → 4 278-byte info blob (device pushes it right after the client's time write) |
| 0x06 | 1 | Time negotiation (no data response) |
| — (CP-68) | — | Client writes its clock: two 5×u32 `y,m,d,h,min` groups — UTC and local time (23:04 and 19:04 in this capture) |
| 0x02 | 4 | File open/stat by path at offset 32; response header `[16:20]` = file size (absent files → 0 + `0x0A1D`; both `0:/log/` and `0:/lgo/splash.bmp` verified live) |
| 0x02 | 2 | Get channel catalog (no path) → 12 812-byte `0x686868` block |
| 0x24 | 6 | List user profiles → 5 × 64-byte entries (316–320 B across boots) |
| 0x24 | 2 | List saved datalogs → CSV, below |
| 0x08 | 2 | Get 2 724-byte `0x686869` block (channel tree) |
| 0x09 | 2 | Get 8 020-byte `0x686866` block (live-frame schema) |
| 0x28 | 6 | Start live stream (no data response) |
| 0x51 | 2 | Stop live stream when `0xffffffff` is set at offset 32; without the flag it is a valid no-op (`0x0A11`, len 0) |
| 0x03 | 2 | Live "main" object. First session after power-on: 9-byte current-profile string `System\0\0\x15` (suffix byte varies: 0x15/0x01 observed). Later sessions: 496-byte live frames — even before NC 0x28/6 and after NC 0x51/2 |
| 0x53 | 2 | Live heartbeat object → 8-byte empty stream frame |
| 0x04 | 2 | Live value snapshot → 204-byte `<hiMST`-framed body that updates in real time (5 bytes changed across a 2 s re-read) |
| 0x52, 0x54 | 2 | Valid but empty (`0x0A11`, len 0) |
| other ids | 2 | 0x01, 0x05, 0x07, 0x0A, 0x25–0x29 → no response at all (client timeout); 0x24/4 and 0x03/6 likewise |
| — (CP-4) | — | u32le file offset → download chunk; `00000000` → generic ack |

## Device info blob (4 279 bytes)

Response to NC `0x10/1`. Uses the same record framing as the channel blocks
and `.xrk` files, with four-character tags that start with `i` and an `a`
marker (corrected 2026-10-02 from a real blob; every checksum verified):

```
'<' 'h' 'i' <TAG:3> <len:u32le> 'a' '>' <payload> '<' 'i' <TAG:3> <sum16:u16le> '>'
```

Text records hold CRLF-separated `key=value|` lines (each value ends in `|`).
`USR` wraps its lines in an ASCII header and trailer, `<hUSR 0000000092a>`
and `<USR 008273>`.

Records present (tags without the `i`):

| TAG | len | Contents |
| --- | --- | --- |
| `MST` | 192 | Nested `idn` subrecords (same `idn` bytes as the UDP descriptor) |
| `HW ` | 57 | `WiFi=ESP32\|Reg=usa\|LSM6DSV16X\|Led=PI33TB\|MYC68B\|M101\|` — Wi-Fi chip, regulatory region, IMU, LED driver, board id |
| `USR` | 124 | Nested `<hUSR ...>` frame plus CRLF-separated `key=value` profile metadata: `device=`, `pilota=` (driver), `veicolo=` (vehicle), `campionato=`, `venue_type=`, `desired_racem=`, `vehicle_type=` — all empty here |
| `PTH` | 1187 | Device path map, CRLF lines `name=dir,index-suffix`, e.g. `media=0:,0:.N`, `settings=0:/set,0:/set.N`, `splash=0:/lgo,0:/lgo.N`, `channels=0:/ch,0:/ch.N`, `overlay=0:/ov,...`, `can1stream=0:/cn1,...`, `kline1stream=0:/kl1,...`, `rs232_1stream=0:/rs1,...`, `mathchannels=0:/mth,...`, `smarty=0:/smc,...`, `shiftlights=0:/shf,...`, `leds=0:/led,...`, `outputs=0:/out,...`, `messages=0:/msg,...`, `popups=0:/pop,...`, `dsplmeas=0:/dsm,...`, `tracks=0:/gps,...`, `tkk=0:/tkk,...`, `canoutput1=0:/cno1,...` |
| `LCK` | 47 | Lock state (not decoded) |
| `SST` | 219 | Not decoded |
| `LTS` | 80–81 | Not decoded |
| `PRL` | 2212 | Not decoded |

Device paths seen elsewhere: `0:/log/splash.bmp` and `0:/lgo/splash.bmp`
(boot logo; both stat to 0), `0:/tkk/dev.ria` (track DB index),
`0:/tkk/p/<8-char-id>.tkk` (per-track data), `1:/mem/a_NNNN.xrz`
(recordings on internal memory).

## Channel catalog, tree, and live-frame schema

Three large blocks share the XRK record framing used inside `.xrk`/`.xrz`
files (same layout `crates/overlay-core/src/xrk.rs::parse_header` accepts):
`<h` + 4-char token + `u32le len` + byte + `>` payload `<` token + `u16le sum` + `>`.
On the wire each block is prefixed by a 4-byte magic and a 4-byte hash:

| Magic | Hash | Size | Delivered by | Content |
| --- | --- | --- | --- | --- |
| `68 68 68 01` | `0432e967` | 12 812 B | NC 0x02/2 | Channel catalog: 97 `<hM…>` records, 112 bytes each |
| `68 68 69 01` | `9c0a4518` | 2 724 B | NC 0x08/2 | 97 `<hM…>` records of 8 bytes (`ffffffff 00000000`) — channel tree/flags |
| `68 68 66 01` | `4c1f51f9` | 8 020 B | NC 0x09/2 | Live-frame schema: 97 `<hM…>` records, 16–188 bytes each |

Catalog record fields (partially decoded): short name at payload offset 24
and long name from offset 32 (`MClk`/`Master Clk`, `LAP`/`Lap Time`,
`RPM`, `WSpd`/`WheelSpeed`, `gpsSt`/`GPS Status`,
`InlA`/`GPS_Pro_InlineAcc`, …), manufacturer `@AIM` followed by a
per-channel id (e.g. `0x1003` for MClk) and a `100000` constant. The 97
entries span timing (0–15), temps/voltages (16–27), RPM/speed (28–31),
accelerometers/gyros (20–25), GPS_Pro derivations (69–81), and generic
expansion channels (48–58, 82–91).

Schema records are indexed by catalog position (`[0:4]` = 0…96) in two
shapes: a 16-byte form `index, param, 0, 0` and a 36-byte form
`index, 0x14, 1, …, float32 0.001 at [20:24], …` (a per-channel scale —
those channels stream as milli-units); a few channels use 132–188-byte
records with extra float fields (bounds). Catalog and schema blocks were
byte-identical across a device power cycle (sha256 `c96cd8b9…` /
`99070661…` over the bodies).

The **first live frame of a streaming session repeats the schema magic and
hash** (`68686601 4c1f51f9`), binding the frame layout to the schema block
delivered by NC 0x09/2.

## Live streaming

After NC `0x28/6` (start), the client alternated two polls roughly every
135 ms (each object ≈ 3.7 Hz) for ~22 s:

* NC `0x03/2` → 500-byte message: 4-byte status + 496-byte frame
* NC `0x53/2` → 12-byte message: status + 8-byte empty frame

The first frame of a session carries the schema binding (`68 68 66 01
4c 1f 51 f9` + the same u32 uptime tick at `[8:12]`); regular frames:

```
6b 6b 6b 01        "kkk" magic
00 00 00 00
<u32le tick_ms>    device uptime milliseconds, frame[8:12]
00..00             slot area (zeros while the car is stationary)
f2 fe cf ff ...    repeated filler pattern, then
12-byte units from 0x7c: <u16> <u32le tick> <u16 0x6321> <u16 0x629ee>
                   (unit ticks track the frame tick; the constant pair
                   0x6321/0x629ee also appears in user-list entries)
...                ~16 hash-like bytes near 0xd8, sparse fields to 0x1e8
```

The heartbeat frame is just `6b6b6b01 00000000`. While stationary, no GPS
position values were recognizable at any alignment (×1e7 fixed, float32,
or ECEF-centimeter forms), so the slot-to-channel mapping — presumably
derivable from the schema block — remains open. Object `0x04/2` returns a
compact live snapshot in `<hiMST` framing that changes between reads.

## Datalog list (CSV)

NC `0x24/2` returns a body that is plain CSV, `name,size,date,hour,...`
rows CRLF-terminated, 60 files in this capture:

```
name,size,date,hour,nlap,nbest,best,pilota,track_name,veicolo,campionato,
venue_type,mode,trk_type,motivolap,maxvel,device,track_lat,track_lon,
test_dur,pname,ptype,ptime,pdist,pmaxv,valid,

a_0089.xrz,477678,30/08/2026,15:33:28,2,,,,,,,,speed,closed,stop,
1079717068,,420123456,-710654321,141383,,,,,,,
```

Notes: sizes are bytes and match the download exactly; `track_lat/lon` are
degrees × 1e7 (42.0123456, −71.0654321; coordinates in this document
are replaced with made-up values); `best` is milliseconds
(`a_0034.hrz,…,best=36605` = 36.605 s at track `KELLYS`); extensions are
`.xrz` (current) and `.hrz` (older), both zlib-wrapped XRK; `maxvel` holds a ~1.08e9 raw value on
every row — read as float32 bits the values are 3.13–3.44, and ×20 ≈
63–69 mph, the right ballpark for these laps (a_0089's GPS top speed is
67.6 mph) — plausible but unconfirmed. Related reads observed just before
the list:
`0:/tkk/dev.ria` → 384-byte track index (`Yard`, hashed ids, `KELLYS`) and
`0:/tkk/p/nkdwremp.tkk` → 6 516-byte track points block (`<hPtkk`, `<Vnfo`,
`<Vidx`, `<hpts` records). Neither decoded further.

## File download

Session 2 in full (this is the entire session, 12 client + 14 device frames):

1. TCP connect; CP-8 hello `06 08` → `06 09`.
2. NC `0x02/4` with path `1:/mem/a_0089.xrz`:
   echo ack, then response header with `len@16 = 0x000749ee` = **477 678**
   (the file size, confirmed byte-for-byte by the transfer).
3. The device **immediately pushes chunk 0** — no read request needed:
   `CP` 65 476 bytes = `u32le offset 0` + 65 472 data bytes.
4. The client requests each further chunk with a 4-byte `CP` frame holding
   the next `u32le offset`; offsets advance by 0xFFC0 = 65 472:
   `0x0000FFC0, 0x0001FF80, 0x0002FF40, 0x0003FF00, 0x0004FEC0, 0x0005FE80,
   0x0006FE40`. Every reply is `u32le offset echo` + up to 65 472 data bytes;
   the final reply carries 19 374 bytes.
5. Transfer total: 7 × 65 472 + 19 374 = **477 678 bytes** — complete, no
   gaps, no retransmission, all checksums valid.
6. Six seconds later the client re-listed files (NC `0x24/2`) on the same
   connection and closed.

Re-verified live: opened `a_0092.xrz` with size 542 400 (matching
its CSV row), chunks at offsets 0x0…0x7FE00 (8 × 65 472 + 19 136), byte
count exact, all checksums valid. Neither the k=1 time-sync rounds nor
anything beyond the CP-8 hello is required — hello, open, offset reads
suffice.

### The `.xrz` container

The downloaded bytes start `78 01` — a **zlib stream**. Inflating yields an
XRK-format recording: `<hCNF…>` (config), `<hCHS…>` (channel set), then the
sample packets — exactly the framing `crates/overlay-core/src/xrk.rs` parses.
Verified end-to-end by inflating the captured chunks and running
`cargo run -p overlay-core --example xrk_inspect` on the result:

* 32 channels, 2 laps (0.0–115.18 s out, 115.18–141.20 s in)
* GPS spanning about 500 m, up to 30.2 m/s, RPM to 15 011
* metadata `date=08/30/2026`, `time=15:33:28` — matches the CSV row exactly.

Second file verified live (`a_0092.xrz`, 542 400 B → 1 084 248 B inflated):
32 channels, 2 laps, GPS spanning about 500 × 360 m,
RPM ≤ 14 378.

So Wi-Fi-downloaded `.xrz` = `zlib_deflate(XRK)`; the app's existing
`.xrk` path can read them after a zlib inflate (the adapter currently gates
on the `.xrk` extension before parsing).

## Live verification (2026-09-29)

All documented operations were re-run against the same device over SSH
from a second test computer (dongle associated to the logger's hotspot, DHCP
`11.0.0.2`) using a standalone client written from this document alone
(`~/aim_client.py` on that host; raw artifacts in `~/mychron_probe/`).
Completed without device errors: hello, both time-sync rounds, info blob,
user list, current-user string, CSV list, catalog/tree/schema, 15 s of
live streaming (106 frames), stream stop, full chunked download, and
fourteen single-op object probes — every frame's checksum validated.

Confirmed exactly as captured: statuses `0x0A09`/`0x0A11`/`0x0A1D`;
header `len@16` semantics; auto-pushed chunk 0 after the header ack;
`0xFFC0` stride; 236-byte UDP descriptor; byte-identical catalog/schema.
Refinements found by testing are folded into the sections above (ack
ordering, `System` first-session-only behavior, splash paths, object
space, `0x51` without the stop flag, silent connections).

## Open questions

* Exact live-frame slot-to-channel mapping: schema record structure is
  decoded (see above) but the `param` semantics are not proven against
  frame contents, and stationary frames show no recognizable GPS values.
* NC ids beyond the documented set behave as fixed slots (invalid ones
  simply never answer); whether ids are stable across firmware versions
  is unknown.
* `MST`/`idn` subrecord fields, `LCK`/`SST`/`LTS`/`PRL` info records,
  track DB (`dev.ria`, `.tkk`) formats.
* UDP descriptor bytes `0x0E`/`0x11` (persisted, meaning unknown);
  `maxvel` unit (float32-mph hypothesis above); the `System` response's
  trailing byte (0x15 vs 0x01); the no-hello frame variant that failed
  our checksum check; what NC `0x28/6` actually changes, since live
  frames flow without it once the schema has been fetched once per boot.

## Bluetooth (BLE) status

Investigated 2026-09-29 against the same unit. The logger's BT identity is
`MYC6-<serial>` with a MAC one above the Wi-Fi MAC; it pairs without a PIN.

GATT table when connected:

| Service | Characteristics | Behavior observed |
| --- | --- | --- |
| Nordic UART `6E400001-…` | `…02` (write), `…03` (read/notify) | **Loopback**: bytes written to `…02` are echoed back on `…03` (`aim-ka`, `<hSTCP` fragments, raw bytes all echoed verbatim). No protocol behind it. |
| Vendor `7E1C2F90-…` | `…91` (write), `…92` (read/notify/indicate) | Writes are ACKed at the ATT layer but never produce any notification or readable value — dormant. UUID absent from public Bluetooth databases. |
| Device Information `0x180A` | Model `MYC6-<serial>`, Manufacturer `AiM Tech Srl` | Standard. |
| GAP `0x1801` | Service Changed (indicate) | Standard. |

Probes that produced no response on any characteristic: framed and raw STCP
hello, the `NC` file-list request, writes fragmented across multiple ≤20-byte
ATT writes, `aim-ka`, short/byte pokes, poll-reads after writes.

The official user manual (§4.6) resolves this: the MyChron6's BLE is a
**central-role sensor link** — only heart-rate monitors of any brand are
"actually enabled", with other devices "to be added in the future". So the
vendor GATT server is reserved future surface (or scaffolding), the NUS
loopback is debug/example leftover, and there is no client-facing BT protocol
in the current firmware. The BT data path in use today is the standard
Bluetooth Heart Rate Profile consumed by the logger as central.

A heart-rate emulator was built to test the ingest path
(`tools/mychron/hrm_peripheral.py`, a BlueZ peripheral advertising the Heart
Rate Service with a cycling value). It was confirmed on air and connectable
from a phone, but the logger's *Select BLT device* scanner never listed it —
including with the advert compacted so name and UUID fit the primary ADV
packet. Likely explanations for a future session: the logger's ESP32 shares
one radio between the Wi-Fi hotspot and BT, and its hotspot was active during
testing (retry with the logger's Wi-Fi set to OFF, available in the US
firmware), or its scanner filters more strictly than the standard profile
requires. The dormant vendor service can only be analyzed once AiM ships a
client that speaks it.

Protocol tools live untracked in `tools/mychron/`: a Wi-Fi STCP client
(`aim_client.py`), the BLE heart-rate emulator (`hrm_peripheral.py`), and a
GATT enumeration/probe script (`bt_gatt_probe.py`).

## Implementation

`crates/overlay-logger` implements everything above in Rust: framing and the
transaction model, every typed operation in the table (time sync, device info,
user profiles, datalog list, channel catalog/tree/schema, live start/stop,
frames, heartbeat and snapshot, file stat and chunked reads), the UDP
descriptor and keepalive, and Wi-Fi control for joining the hotspot. Parts this
document leaves undecoded are returned as raw, checksum-annotated records. Its
`mychron` example is a command-line client (`discover`, `probe`, `list`,
`download`, `read`, `stat`, `info`, `set-clock`, `live`, `object`, `wifi …`)
that supersedes `aim_client.py`; `--addr host[:tcp[:udp]]` or
`RACE_OVERLAY_MYCHRON_ADDR` points it at a relay. The unit tests run against an
in-process fake logger; `tests/device.rs` holds ignored tests for a real one:

```sh
cargo test -p overlay-logger --test device -- --ignored
```

### Rust client verification (2026-10-02)

Run from a Linux host whose second adapter was associated with the logger's
hotspot (`AiM-MYC6-011681`), with the `aim-ka` keepalive running throughout:

* All five `tests/device.rs` tests pass: stable descriptor fingerprint, datalog
  list after the **hello alone** (no time sync), catalog/tree/schema hashes
  identical to the capture (97 channels), 10 polls of live streaming plus
  stop, and a byte-exact download that inflates to XRK.
* **Descriptor length fix:** the descriptor's leading word is the length of
  the whole datagram (0xec = 236), not of what follows it; the transport
  section above originally said "236−4".
* 60 recordings listed; `stat`/`read` of `1:/mem/a_0089.xrz` (477 678 B),
  `0:/tkk/dev.ria` (384 B), and absent `0:/lgo/splash.bmp` as documented;
  NC `0x52/2` empty `0x0A11`; NC `0x05/2` silent.
* `.hrz` recordings (e.g. `a_0034.hrz`, 715 309 B → 1 363 087 B) are the same
  zlib-wrapped XRK as `.xrz` and import the same way. Throughput was about
  160 KB/s (4.5 s for that file).
* Live frames: over 10 s the tick at `[8:12]` advanced about 350 ms per poll.
  In one run started right after an unanswered NC `0x05/2` probe, nine frames
  over 3 s all repeated the same tick. Cause unknown.
* Device info (after a clock write from a host on the same time zone): the
  blob's framing differed from the description above, which has been
  corrected; all eight records decode with valid checksums, and the path map
  has 41 entries (including `recorded=1:/mem`, where recordings live).
* The app's own service (browse, download, verify, library, import, and Track
  mode limited to today) passed against the logger
  (`real_logger_browse_download_and_track`, ignored by default).

* **Auto-off timer** (set on the logger; 2 minutes minimum). Observations:
  * With `aim-ka` keepalives every 1.1 s, all answered, the logger still
    switched off.
  * Run 1 (10 minutes): powered off about 10 minutes after a burst of
    downloads, file reads, channel-block reads, and live streaming ending at
    17:28, and about 14 minutes after power-on.
  * Run 2 (10 minutes): powered off about 10 minutes after power-on and a
    clock write, despite a second device-info read (with clock write) 2 minutes
    later and datalog list requests 5 and 8 minutes later.
  * Run 3 (2 minutes): powered off about 2 minutes after power-on despite NC
    `0x53/2` heartbeats every 20 s.
  * Run 4 (2 minutes): a short live stream every 20 s (schema, NC `0x28/6`,
    about 1 s of NC `0x03/2` + `0x53/2` polls, NC `0x51/2` stop) kept it on
    for 5 minutes.
  * Run 5 (2 minutes): NC `0x03/2` alone every 20 s, on its own connection,
    kept it on for a further 5 minutes.

  So keepalives, the datalog list, time sync, and the live heartbeat do not
  restart the timer; reading the live "main" object (NC `0x03/2`) does. The
  app polls it every 30 s while its window shows a connected logger. Its
  discovery probes alone do not keep a parked logger awake.

Still open:

* listing and downloading while the logger is recording;
* joining and leaving the hotspot from macOS (CoreWLAN).

## Appendix: minimal frame parser (Python)

```python
def frames(buf):
    """Yield (tag, payload, sum16) for every STCP frame in a TCP byte stream."""
    i = 0
    while True:
        j = buf.find(b'<hST', i)
        if j < 0:
            return
        tag = buf[j+4:j+6]
        if tag in (b'CP', b'NC'):
            n = int.from_bytes(buf[j+6:j+10], 'little')
            if buf[j+10] == 0 and buf[j+11] == 0x3E and j+12+n <= len(buf):
                body = buf[j+12:j+12+n]
                k = buf.find(b'<hST', j+12+n)
                trailer = buf[j+12+n:k if k > 0 else len(buf)]
                # trailer == b'<ST' + tag + u16le(sum(body)) + b'>'
                yield tag, body, int.from_bytes(trailer[5:7], 'little')
                i = j + 12 + n
                continue
        i = j + 4
```
