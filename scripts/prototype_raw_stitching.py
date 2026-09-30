#!/usr/bin/env python3
"""Explicit OpenCV stitching experiment; no application/runtime dependency.

Use the optical.json and lens PNGs produced by inspect_raw_video. This measures
fixed camera-coordinate overlap preparation, excluding decode, projection and UI.
It does not implement the temporal policy required to register a shipping backend.
Install numpy and opencv-contrib-python in an isolated environment to run it.
"""
import argparse
import json
import time
from pathlib import Path

import cv2
import numpy as np


def overlap(images, lenses, width, height, side):
    latitude = np.linspace(-np.pi / 2 + .04, np.pi / 2 - .04, height)[:, None]
    longitude = np.linspace(side * np.pi / 2 - .25, side * np.pi / 2 + .25, width)[None, :]
    rays = np.stack(np.broadcast_arrays(np.sin(longitude) * np.cos(latitude),
                    np.sin(latitude), np.cos(longitude) * np.cos(latitude)), axis=2)
    samples, masks = [], []
    for image, lens in zip(images, lenses):
        ray = rays @ np.array(lens["rotation"]).T
        denominator = ray[:, :, 2] + lens["xi"]
        x, y = ray[:, :, 0] / denominator, ray[:, :, 1] / denominator
        r2 = x * x + y * y
        k1, k2, k3, p1, p2 = lens["distortion"]
        radial = 1 + r2 * (k1 + r2 * (k2 + r2 * k3))
        u = lens["center"][0] + lens["focal"][0] * (x * radial + 2 * p1 * x * y + p2 * (r2 + 2 * x * x))
        v = lens["center"][1] + lens["focal"][1] * (y * radial + 2 * p2 * x * y + p1 * (r2 + 2 * y * y))
        valid = ((u - .5) ** 2 + (v - .5) ** 2 < .25) & (ray[:, :, 2] > np.cos(np.deg2rad(100)))
        samples.append(cv2.remap(image, (u * image.shape[1] - .5).astype("float32"),
                       (v * image.shape[0] - .5).astype("float32"), cv2.INTER_LINEAR))
        masks.append(valid.astype("uint8") * 255)
    # Keep each primary hemisphere represented at the boundaries.
    masks[0][:, -4 if side == 1 else 0:None if side == 1 else 4] = 0
    masks[1][:, 0 if side == 1 else -4:4 if side == 1 else None] = 0
    return samples, masks


def prepare(images, masks, flow):
    images = [image.copy() for image in images]
    masks = [cv2.UMat(mask.copy()) for mask in masks]
    corners = [(0, 0), (0, 0)]
    exposure = cv2.detail.GainCompensator()
    exposure.feed(corners, images, masks)
    for index in range(2):
        exposure.apply(index, (0, 0), images[index], masks[index])
    confidence = None
    if flow:
        dis = cv2.DISOpticalFlow_create(cv2.DISOPTICAL_FLOW_PRESET_FAST)
        gray = [cv2.cvtColor(image, cv2.COLOR_BGR2GRAY) for image in images]
        forward = dis.calc(gray[0], gray[1], None)
        backward = dis.calc(gray[1], gray[0], None)
        height, width = gray[0].shape
        xx, yy = np.meshgrid(np.arange(width, dtype="float32"), np.arange(height, dtype="float32"))
        reverse = cv2.remap(backward, xx + forward[:, :, 0], yy + forward[:, :, 1], cv2.INTER_LINEAR)
        good = ((np.linalg.norm(forward + reverse, axis=2) < 1)
                & (np.linalg.norm(forward, axis=2) < 6)
                & (masks[0].get() > 0) & (masks[1].get() > 0))
        confidence = float(good.mean())
        forward *= good[:, :, None]  # Retain calibrated geometry where correspondence fails.
        images = [cv2.remap(images[0], xx - forward[:, :, 0] * .5, yy - forward[:, :, 1] * .5, cv2.INTER_LINEAR),
                  cv2.remap(images[1], xx + forward[:, :, 0] * .5, yy + forward[:, :, 1] * .5, cv2.INTER_LINEAR)]
    seam = cv2.detail.GraphCutSeamFinder("COST_COLOR_GRAD")
    seam.find([image.astype("float32") for image in images], corners, masks)
    blender = cv2.detail.MultiBandBlender()
    blender.setNumBands(4)
    height, width = images[0].shape[:2]
    blender.prepare((0, 0, width, height))
    for image, mask in zip(images, masks):
        blender.feed(image.astype("int16"), mask, (0, 0))
    result, valid = blender.blend(None, None)
    result = np.clip(result, 0, 255).astype("uint8")
    result[valid == 0] = 0
    return result, confidence


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("validation_directory", type=Path)
    parser.add_argument("output_directory", type=Path)
    parser.add_argument("--width", type=int, default=96)
    parser.add_argument("--height", type=int, default=256)
    parser.add_argument("--iterations", type=int, default=10)
    args = parser.parse_args()
    if min(args.width, args.height, args.iterations) <= 0:
        parser.error("dimensions and iterations must be positive")
    metadata = json.loads((args.validation_directory / "optical.json").read_text())
    images = [cv2.imread(str(args.validation_directory / f"lens-{lens}.png")) for lens in ("front", "rear")]
    if any(image is None for image in images):
        parser.error("both decoded lens PNGs are required")
    regions = [overlap(images, metadata["lenses"], args.width, args.height, side) for side in (-1, 1)]
    args.output_directory.mkdir(parents=True, exist_ok=True)
    report = {"opencv": cv2.__version__, "region_size": [args.width, args.height],
              "regions": 2, "temporal_policy_validated": False, "methods": {}}
    for flow in (False, True):
        name = "exposure_graphcut_multiband" + ("_dis" if flow else "")
        times = []
        for iteration in range(args.iterations + 1):
            start = time.perf_counter()
            results = [prepare(*region, flow) for region in regions]
            elapsed = (time.perf_counter() - start) * 1000
            if iteration:
                times.append(elapsed)
        for index, (result, _) in enumerate(results):
            cv2.imwrite(str(args.output_directory / f"{name}-{index}.png"), result)
        report["methods"][name] = {"median_ms": float(np.median(times)), "p95_ms": float(np.percentile(times, 95)),
                                   "accepted_flow_fraction": [confidence for _, confidence in results]}
    (args.output_directory / "report.json").write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))


if __name__ == "__main__":
    main()
