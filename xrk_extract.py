#!/usr/bin/env python3
"""
XRK extractor (v2): parses <hXYZ> blocks, lists channels, GPS raw, and
now **detects and inflates embedded zlib streams** inside any block payload.

Design goals:
- DO NOT misinterpret raw data as compressed: we only mark a payload "compressed"
  when a zlib stream starting at a recognized header (0x78 01/5E/9C/DA) successfully
  inflates via Python's zlib with no errors and consumes a valid span of bytes.
- Support multiple embedded streams inside a single payload.
- Preserve raw payload views even when inflated streams exist.

Outputs:
- catalog.csv                    — per-block offset/code/length
- channels.csv                   — CHS block strings
- gps_raw.csv                    — raw GPS numeric previews
- <code>_blocks.csv              — overview of all blocks for each code
- compressed_index.csv           — index of all detected compressed substreams
- inflated/<code>_<i>_<off>.bin  — decompressed bytes (one file per substream)

Usage:
    python xrk_extract.py INPUT.xrk [OUTPUT_DIR]
"""
import sys, struct, json, zlib
from pathlib import Path
import pandas as pd

ZLIB_HEADERS = {b"\x78\x01", b"\x78\x5e", b"\x78\x9c", b"\x78\xda"}

def parse_all_blocks(blob: bytes):
    blocks = []
    i = 0
    while i < len(blob)-16:
        """ Blocks are tagged with something like <hVEL......>, where . is not printable
        often there's a byte that follows the 3 letter string, maybe a checksum?
        """
        if blob[i:i+2] == b"<h" and all(ord('A') <= c <= ord('Z') for c in blob[i+2:i+5]):
            code = blob[i+2:i+5].decode('ascii')
            length = struct.unpack_from("<I", blob, i+6)[0]
            marker = struct.unpack_from("<I", blob, i+10)[0]
            payload_start = i+14
            payload = blob[payload_start:payload_start+length]
            blocks.append({"offset": i, "code": code, "length": length, "marker": marker, "payload_start": payload_start, "payload": payload})
            i = payload_start + length
        else:
            i += 1
    return blocks

def extract_strings(b: bytes, min_len=3):
    s = ''.join(chr(c) if 32 <= c < 127 else '\x00' for c in b)
    return [p for p in s.split('\x00') if len(p) >= min_len]

def find_zlib_streams(payload: bytes):
    """Yield tuples (start_off, consumed_len, out_bytes) for each successful zlib stream.
    Conservative approach:
      - Only consider offsets where two bytes match a known zlib header.
      - Use zlib.decompressobj() (zlib-wrapped) and require successful inflate.
      - Require positive output length and that some input was consumed.
      - Advance cursor by consumed bytes to avoid nested duplicates.
    """
    streams = []
    i = 0
    L = len(payload)
    while i+2 <= L:
        # Scan to next header candidate
        if payload[i:i+2] not in ZLIB_HEADERS:
            i += 1
            continue
        # Attempt inflate from here
        try:
            decomp = zlib.decompressobj()  # zlib wrapper
            out = decomp.decompress(payload[i:])
            # When stream ends, unused_data is the remainder after the stream
            unused = decomp.unused_data
            consumed = len(payload[i:]) - len(unused)
            # Validate
            if consumed <= 6:  # too short to be meaningful (zlib hdr + tiny body + adler32 is > 6, but be conservative)
                i += 1
                continue
            if len(out) == 0:
                i += 1
                continue
            # Additional sanity: zlib must be at least header(2) + adler32(4) = 6, we already check consumed>6
            streams.append((i, consumed, out))
            # Advance past this stream to find additional streams
            i += consumed
            continue
        except zlib.error:
            # Not a valid zlib stream starting here; move on
            i += 1
            continue
    return streams

def run(input_path: Path, outdir: Path):
    outdir.mkdir(parents=True, exist_ok=True)
    blob = input_path.read_bytes()
    blocks = parse_all_blocks(blob)

    # Catalog
    catalog = pd.DataFrame([{"offset": b["offset"], "code": b["code"], "length": b["length"], "marker": b["marker"]} for b in blocks])
    catalog.to_csv(outdir/"catalog.csv", index=False)

    # Channels (CHS)
    chs_rows = []
    for b in blocks:
        if b["code"] == "CHS":
            parts = extract_strings(b["payload"], 2)
            short = parts[0] if len(parts)>=1 else ""
            long = parts[1] if len(parts)>=2 else ""
            chs_rows.append({"offset": b["offset"], "short": short, "name": long, "strings": parts})
    if chs_rows:
        pd.DataFrame(chs_rows).to_csv(outdir/"channels.csv", index=False)

    # GPS raw numeric views
    gps_rows = []
    for b in blocks:
        if b["code"] == "GPS":
            pay = b["payload"]
            N = (len(pay)//4)
            ints = list(struct.unpack_from("<" + "i"*N, pay[:N*4])) if N>0 else []
            uints = list(struct.unpack_from("<" + "I"*N, pay[:N*4])) if N>0 else []
            floats = list(struct.unpack_from("<" + "f"*N, pay[:N*4])) if N>0 else []
            row = {"offset": b["offset"], "payload_len": len(pay)}
            # include a timestamp-looking u64 if present
            if len(pay) >= 8:
                row["ts_u64"] = struct.unpack_from("<Q", pay, 0)[0]
            for i,v in enumerate(ints[:12]): row[f"i{i}"]=v
            for i,v in enumerate(uints[:12]): row[f"u{i}"]=v
            for i,v in enumerate(floats[:12]): row[f"f{i}"]=v
            gps_rows.append(row)
    if gps_rows:
        pd.DataFrame(gps_rows).to_csv(outdir/"gps_raw.csv", index=False)

    # Per-code payload overviews
    code_groups = {}
    for b in blocks:
        code_groups.setdefault(b["code"], []).append(b)

    for code, group in code_groups.items():
        rows = []
        for idx, b in enumerate(group):
            strings = extract_strings(b["payload"], 3)
            rows.append({
                "offset": b["offset"],
                "length": b["length"],
                "strings_preview": "|".join(strings[:8]),
                "payload_hex_prefix": b["payload"][:64].hex()
            })
        pd.DataFrame(rows).to_csv(outdir/f"{code.lower()}_blocks.csv", index=False)

    # Detect + inflate embedded zlib streams
    comp_rows = []
    infl_dir = outdir / "inflated"
    infl_dir.mkdir(exist_ok=True)
    for bi, b in enumerate(blocks):
        payload = b["payload"]
        streams = find_zlib_streams(payload)
        for si, (start_off, consumed, out_bytes) in enumerate(streams):
            # Save decompressed out bytes
            fn = f"{b['code'].lower()}_{b['offset']}_{start_off}.bin"
            fpath = infl_dir / fn
            fpath.write_bytes(out_bytes)
            comp_rows.append({
                "block_offset": b["offset"],
                "code": b["code"],
                "payload_len": len(payload),
                "zlib_start_in_payload": start_off,
                "zlib_consumed_bytes": consumed,
                "inflated_bytes": len(out_bytes),
                "inflated_file": fn
            })
    if comp_rows:
        pd.DataFrame(comp_rows).to_csv(outdir/"compressed_index.csv", index=False)

    # Metadata
    meta = {"source": input_path.name,
            "total_blocks": len(blocks),
            "codes": {code: len(group) for code, group in code_groups.items()}}
    (outdir/"meta.json").write_text(json.dumps(meta, indent=2))

if __name__ == "__main__":
    import sys
    if len(sys.argv) < 2:
        print("Usage: python xrk_extract.py INPUT.xrk [OUTPUT_DIR]")
        raise SystemExit(1)
    inp = Path(sys.argv[1])
    out = Path(sys.argv[2]) if len(sys.argv) > 2 else inp.with_suffix("").with_name(inp.stem + "_extracted_v2")
    run(inp, out)
