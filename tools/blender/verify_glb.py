#!/usr/bin/env python3
"""verify_glb.py -- parse a binary glTF (.glb) and audit it against the KimiOS
software-rasteriser contract.

Runs under plain CPython *and* inside Blender's bundled Python (no bpy needed).

    python tools/blender/verify_glb.py assets/models/testchar.glb

Checks (and prints):
  * GLB container: magic / version / declared length / actual length / chunks
  * primitives: POSITION, NORMAL, TEXCOORD_0, JOINTS_0, WEIGHTS_0, indices
  * joints: max joint index, weights normalised, influences per vertex
  * skins: inverseBindMatrices accessor (MAT4, one per joint)
  * images: embedded in the BIN chunk (bufferView, no URI), mimeType, PNG size
  * nodes: humanoid bone names survived export
Exit code 0 if every required feature is present, 1 otherwise.
"""
import json
import os
import struct
import sys

COMPONENT = {5120: ("b", 1), 5121: ("B", 1), 5122: ("h", 2), 5123: ("H", 2), 5125: ("I", 4), 5126: ("f", 4)}
NCOMP = {"SCALAR": 1, "VEC2": 2, "VEC3": 3, "VEC4": 4, "MAT2": 4, "MAT3": 9, "MAT4": 16}
COMPONENT_NAME = {5120: "BYTE", 5121: "UNSIGNED_BYTE", 5122: "SHORT", 5123: "UNSIGNED_SHORT", 5125: "UNSIGNED_INT", 5126: "FLOAT"}

HUMANOID_BONES = [
    "hips", "spine", "chest", "neck", "head",
    "leftUpperArm", "leftLowerArm", "leftHand",
    "rightUpperArm", "rightLowerArm", "rightHand",
    "leftUpperLeg", "leftLowerLeg", "leftFoot",
    "rightUpperLeg", "rightLowerLeg", "rightFoot",
]


# --------------------------------------------------------------------------- glb
def parse_glb(path):
    """Return dict(header, json, bin, size, chunks)."""
    with open(path, "rb") as f:
        data = f.read()
    if len(data) < 12:
        raise ValueError("file too small to be a GLB: %d bytes" % len(data))
    magic, version, length = struct.unpack_from("<4sII", data, 0)
    if magic != b"glTF":
        raise ValueError("not a GLB: magic=%r (expected b'glTF')" % magic)
    chunks = []
    off = 12
    gltf = None
    binchunk = b""
    while off + 8 <= min(length, len(data)):
        clen, ctype = struct.unpack_from("<I4s", data, off)
        payload = data[off + 8:off + 8 + clen]
        chunks.append((ctype, clen))
        if ctype == b"JSON":
            gltf = json.loads(payload.decode("utf-8"))
        elif ctype in (b"BIN\x00", b"BIN "):
            binchunk = payload
        off += 8 + clen
    if gltf is None:
        raise ValueError("GLB has no JSON chunk")
    return {
        "header": {"magic": magic, "version": version, "declared_length": length,
                   "actual_length": len(data)},
        "json": gltf, "bin": binchunk, "size": len(data), "chunks": chunks,
    }


def accessor_data(gltf, binchunk, index):
    """Decode one accessor into a list of tuples (float/int)."""
    acc = gltf["accessors"][index]
    ct, typ = acc["componentType"], acc["type"]
    if ct not in COMPONENT:
        raise ValueError("unsupported componentType %r" % (ct,))
    fmt, csize = COMPONENT[ct]
    n = NCOMP[typ]
    count = acc["count"]
    if acc.get("sparse"):
        raise ValueError("sparse accessors not handled")
    if "bufferView" not in acc:
        return [(0.0,) * n] * count
    bv = gltf["bufferViews"][acc["bufferView"]]
    base = bv.get("byteOffset", 0) + acc.get("byteOffset", 0)
    elem = csize * n
    stride = bv.get("byteStride") or elem
    out = []
    for i in range(count):
        off = base + i * stride
        out.append(struct.unpack_from("<" + fmt * n, binchunk, off))
    return out


def png_size(blob):
    """Return (width, height) from a PNG's IHDR, or None."""
    if len(blob) < 24 or blob[:8] != b"\x89PNG\r\n\x1a\n":
        return None
    if blob[12:16] != b"IHDR":
        return None
    w, h = struct.unpack_from(">II", blob, 16)
    return w, h


# --------------------------------------------------------------------------- audit
def audit(path, verbose=True):
    rep = {"path": path, "errors": [], "warnings": [], "ok": False}
    g = parse_glb(path)
    gltf, binc, hdr = g["json"], g["bin"], g["header"]
    rep.update(gltf=gltf, header=hdr, size=g["size"], chunks=g["chunks"])

    if hdr["declared_length"] != hdr["actual_length"]:
        rep["errors"].append("header length %d != file size %d" % (hdr["declared_length"], hdr["actual_length"]))

    meshes = gltf.get("meshes", [])
    skins = gltf.get("skins", [])
    nodes = gltf.get("nodes", [])
    node_names = [n.get("name", "") for n in nodes]

    # ---- primitives
    prims = []
    tris = 0
    for mi, mesh in enumerate(meshes):
        for pi, prim in enumerate(mesh.get("primitives", [])):
            attrs = prim.get("attributes", {})
            idx = prim.get("indices")
            nidx = gltf["accessors"][idx]["count"] if idx is not None else 0
            ntris = nidx // 3
            tris += ntris
            pos_idx = attrs.get("POSITION")
            nverts = gltf["accessors"][pos_idx]["count"] if pos_idx is not None else 0
            attr_detail = {}
            for aname, ai in sorted(attrs.items()):
                acc = gltf["accessors"][ai]
                attr_detail[aname] = {
                    "accessor": ai, "type": acc["type"],
                    "componentType": acc["componentType"],
                    "component": COMPONENT_NAME.get(acc["componentType"], str(acc["componentType"])),
                    "count": acc["count"], "normalized": acc.get("normalized", False),
                }
            prims.append({"mesh": mi, "prim": pi, "attrs": sorted(attrs), "tris": ntris,
                          "verts": nverts, "mode": prim.get("mode", 4), "attr_detail": attr_detail})
    rep["prims"] = prims
    rep["triangles"] = tris
    rep["vertices"] = sum(p["verts"] for p in prims)
    rep["accessors"] = len(gltf.get("accessors", []))

    required_attrs = ["POSITION", "NORMAL", "TEXCOORD_0", "JOINTS_0", "WEIGHTS_0"]
    for p in prims:
        for a in required_attrs:
            if a not in p["attrs"]:
                rep["errors"].append("mesh %d prim %d missing attribute %s" % (p["mesh"], p["prim"], a))
        if p["mode"] != 4:
            rep["errors"].append("mesh %d prim %d mode=%r (expected 4/TRIANGLES)" % (p["mesh"], p["prim"], p["mode"]))
        if p["tris"] >= 4000:
            rep["errors"].append("triangle budget exceeded: %d >= 4000" % p["tris"])
    if not prims:
        rep["errors"].append("no mesh primitives")

    # ---- positions / normals
    pos_min, pos_max = None, None
    for mi, mesh in enumerate(meshes):
        for prim in mesh.get("primitives", []):
            pi = prim["attributes"].get("POSITION")
            if pi is None:
                continue
            acc = gltf["accessors"][pi]
            mn = acc.get("min")
            mx = acc.get("max")
            if mn and mx:
                pos_min = mn if pos_min is None else [min(a, b) for a, b in zip(pos_min, mn)]
                pos_max = mx if pos_max is None else [max(a, b) for a, b in zip(pos_max, mx)]
    rep["bbox_min"], rep["bbox_max"] = pos_min, pos_max

    # ---- skins
    rep["skins"] = []
    for si, skin in enumerate(skins):
        joints = skin.get("joints", [])
        ibm = skin.get("inverseBindMatrices")
        info = {"index": si, "joint_count": len(joints), "ibm_accessor": ibm,
                "joint_names": [nodes[j].get("name", "<unnamed>") for j in joints]}
        if ibm is None:
            rep["errors"].append("skin %d has no inverseBindMatrices" % si)
        else:
            acc = gltf["accessors"][ibm]
            info["ibm_type"], info["ibm_count"] = acc["type"], acc["count"]
            if acc["type"] != "MAT4":
                rep["errors"].append("skin %d inverseBindMatrices type=%s (expected MAT4)" % (si, acc["type"]))
            if acc["count"] != len(joints):
                rep["errors"].append("skin %d IBM count %d != joint count %d" % (si, acc["count"], len(joints)))
        rep["skins"].append(info)
    if not skins:
        rep["errors"].append("no skins")
    if len(skins) > 1:
        rep["warnings"].append("%d skins (loader may expect exactly 1)" % len(skins))

    # ---- joint indices / weights / influences
    rep["influence_histogram"] = {}
    rep["max_influences"] = 0
    max_joint_index = -1
    weight_sum_bad = 0
    for ni, mesh in enumerate(meshes):
        for prim in mesh.get("primitives", []):
            jidx = prim["attributes"].get("JOINTS_0")
            widx = prim["attributes"].get("WEIGHTS_0")
            if jidx is None or widx is None:
                continue
            jdata = accessor_data(gltf, binc, jidx)
            wdata = accessor_data(gltf, binc, widx)
            for joints, weights in zip(jdata, wdata):
                max_joint_index = max(max_joint_index, max(joints))
                nz = sum(1 for w in weights if w > 1e-6)
                rep["influence_histogram"][nz] = rep["influence_histogram"].get(nz, 0) + 1
                rep["max_influences"] = max(rep["max_influences"], nz)
                s = sum(weights)
                if abs(s - 1.0) > 1e-3:
                    weight_sum_bad += 1
    rep["max_joint_index"] = max_joint_index
    if weight_sum_bad:
        rep["warnings"].append("%d vertices have weights not summing to 1.0" % weight_sum_bad)
    if rep["skins"] and rep["max_joint_index"] >= rep["skins"][0]["joint_count"]:
        rep["errors"].append("joint index %d out of range for %d joints" % (rep["max_joint_index"], rep["skins"][0]["joint_count"]))
    if rep["max_influences"] > 4:
        rep["errors"].append("more than 4 influences per vertex: %d" % rep["max_influences"])

    # ---- images embedded as PNG
    rep["images"] = []
    for ii, im in enumerate(gltf.get("images", [])):
        entry = {"index": ii, "mimeType": im.get("mimeType"), "name": im.get("name"),
                 "uri": im.get("uri"), "bufferView": im.get("bufferView")}
        if im.get("uri"):
            rep["errors"].append("image %d is external (uri=%r); expected embedded" % (ii, im["uri"][:40]))
        elif "bufferView" in im:
            bv = gltf["bufferViews"][im["bufferView"]]
            off, ln = bv.get("byteOffset", 0), bv["byteLength"]
            blob = binc[off:off + ln]
            entry["bytes"] = ln
            entry["png"] = png_size(blob)
            if im.get("mimeType") != "image/png" and not (entry["png"] and entry["mimeType"] is None):
                rep["errors"].append("image %d mimeType=%r (expected image/png)" % (ii, im.get("mimeType")))
            if entry["png"]:
                entry["width"], entry["height"] = entry["png"]
                if max(entry["png"]) > 256:
                    rep["errors"].append("texture %dx%d exceeds 256 px" % entry["png"])
            else:
                rep["errors"].append("image %d is not a valid PNG" % ii)
        rep["images"].append(entry)
    if not rep["images"]:
        rep["errors"].append("no embedded texture image")

    # ---- humanoid bone names
    rep["missing_bones"] = [b for b in HUMANOID_BONES if b not in node_names]
    rep["humanoid_bones_found"] = [b for b in HUMANOID_BONES if b in node_names]
    if rep["missing_bones"]:
        rep["errors"].append("missing humanoid bones: %s" % ", ".join(rep["missing_bones"]))
    if not any(n.get("mesh") is not None for n in nodes):
        rep["warnings"].append("no node references a mesh")

    rep["node_names"] = node_names
    rep["ok"] = not rep["errors"]
    if verbose:
        print_report(rep)
    return rep


def print_report(rep):
    h = rep["header"]
    print("=" * 78)
    print("GLB AUDIT  %s" % rep["path"])
    print("=" * 78)
    print("container   : magic=%r version=%d declared=%d actual=%d bytes  %s"
          % (h["magic"].decode("ascii", "replace"), h["version"], h["declared_length"],
             h["actual_length"], "OK" if h["declared_length"] == h["actual_length"] else "MISMATCH"))
    CHUNK_NAMES = {b"JSON": "JSON", b"BIN\x00": "BIN", b"BIN ": "BIN"}
    print("chunks      : %s" % ", ".join(
        "%s(%d)" % (CHUNK_NAMES.get(c[0], c[0].decode("ascii", "replace").strip("\x00 ")), c[1])
        for c in rep.get("chunks", [])))
    print("counts      : meshes=%d primitives=%d vertices=%d triangles=%d accessors=%d skins=%d images=%d nodes=%d"
          % (len(rep["gltf"].get("meshes", [])), len(rep.get("prims", [])), rep.get("vertices", 0),
             rep.get("triangles", 0), rep["accessors"], len(rep["skins"]),
             len(rep.get("images", [])), len(rep["node_names"])))
    if rep.get("bbox_min"):
        print("bbox        : min=(%.3f, %.3f, %.3f) max=(%.3f, %.3f, %.3f)"
              % (tuple(rep["bbox_min"]) + tuple(rep["bbox_max"])))
    print("-" * 78)
    print("primitives")
    for p in rep["prims"]:
        print("  mesh %d prim %d : %4d tris  %4d verts  mode=%d  attrs=%s"
              % (p["mesh"], p["prim"], p["tris"], p["verts"], p["mode"], ",".join(p["attrs"])))
        for aname in p["attrs"]:
            d = p["attr_detail"][aname]
            print("      %-11s accessor %-3d %-6s %-15s count=%d%s"
                  % (aname, d["accessor"], d["type"], d["component"], d["count"],
                     "  normalized" if d["normalized"] else ""))
    print("-" * 78)
    print("skins")
    for s in rep["skins"]:
        print("  skin %d : %d joints, inverseBindMatrices accessor=%s type=%s count=%s"
              % (s["index"], s["joint_count"], s["ibm_accessor"], s.get("ibm_type"), s.get("ibm_count")))
        print("           joints: %s" % ", ".join(s["joint_names"]))
    print("  max JOINTS_0 index=%d  max influences/vertex=%d  histogram=%s"
          % (rep.get("max_joint_index"), rep.get("max_influences"),
             {str(k): v for k, v in sorted(rep.get("influence_histogram", {}).items())}))
    print("-" * 78)
    print("textures")
    for im in rep.get("images", []):
        print("  image %d : mimeType=%s embedded=%s size=%dx%d bytes=%s name=%r"
              % (im["index"], im.get("mimeType"), im.get("bufferView") is not None,
                 im.get("width", 0), im.get("height", 0), im.get("bytes"), im.get("name")))
    print("-" * 78)
    print("humanoid bones : %d/%d present" % (len(rep["humanoid_bones_found"]), len(HUMANOID_BONES)))
    if rep["missing_bones"]:
        print("  MISSING: %s" % ", ".join(rep["missing_bones"]))
    print("nodes          : %s" % ", ".join(rep["node_names"]))
    print("-" * 78)
    if rep["errors"]:
        for e in rep["errors"]:
            print("ERROR   : %s" % e)
    for w in rep["warnings"]:
        print("WARNING : %s" % w)
    print("RESULT  : %s" % ("PASS" if rep["ok"] else "FAIL"))
    print("=" * 78)


def main(argv=None):
    argv = list(sys.argv[1:] if argv is None else argv)
    if not argv or argv[0] in ("-h", "--help"):
        print(__doc__)
        return 0 if argv else 2
    rc = 0
    for path in argv:
        if not os.path.isfile(path):
            print("ERROR: no such file: %s" % path)
            rc = 1
            continue
        rep = audit(path)
        if not rep["ok"]:
            rc = 1
    return rc


if __name__ == "__main__":
    sys.exit(main())