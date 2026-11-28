# Blender asset pipeline (KimiOS)

Headless Blender scripts that produce the device-ready 3D assets for the `no_std`
software rasteriser:

* `make_test_character.py` — procedurally builds the stylised chibi test character
  (`assets/models/testchar.glb`) and renders proof sheets.
* `vrm_to_kimios.py` — normalises **any** VRM / glTF / GLB / FBX avatar into a GLB
  the KimiOS loader can actually spawn (canonical rig, loader caps, textures,
  expressions).
* `bake_vrm.py` — renders a character into a 4 × 4 sprite pack (2D pet fallback).
* `verify_glb.py` — standalone GLB audit used by every script above.

Everything here is driven by Blender's Python API from the command line. There is
**no Blender MCP server** in this environment and none is needed.

---

## Environment (verified)

| item | value |
|---|---|
| Blender | **5.2.3 LTS** (hash `e13b728c6880`, built 2026-10-22), bundled Python 3.13.13 |
| Blender path | `C:\Program Files\Blender Foundation\Blender 5.2\blender.exe` — **not on PATH** |
| System Python | 3.14.8 (used for `verify_glb.py`; it needs no `bpy`) |
| Host | Windows, headless (`-b`); EEVEE renders fine without a GPU display |

The canonical invocation — `--factory-startup` guarantees no user add-ons,
`--python-exit-code 1` turns an uncaught script exception into a non-zero exit:

```powershell
$bl = "C:\Program Files\Blender Foundation\Blender 5.2\blender.exe"
& $bl -b --factory-startup --python-exit-code 1 --python <script.py> -- <script args>
```

---

## 1. Generate the test character

```powershell
& $bl -b --factory-startup --python-exit-code 1 `
    --python tools/blender/make_test_character.py
```

Default run writes the GLB **and** all three proof renders.

| output | what |
|---|---|
| `assets/models/testchar.glb` | 136,452-byte skinned GLB, 1,310 tris / 931 verts, 3 morphs |
| `assets/models/testchar_contact.png` | 1920×700 four-pose contact sheet (T-POSE / WAVE / WALK / KICK) |
| `assets/models/testchar_beauty.png` | 2160×960 beauty sheet: front, three-quarter, face close-up |
| `assets/models/testchar_app.png` | 798×512 runtime-accurate preview: the model shaded and outlined exactly like the device, shown at 190 / 150 / 120 logical px on light and dark backdrops |
| `assets/models/testchar_expressions.png` | *optional* 1600×500 proof: neutral / blink / happy / surprised |

Options (all after `--`):

| option | default | meaning |
|---|---|---|
| `--out PATH` | `assets/models/testchar.glb` | GLB destination |
| `--contact PATH` | `assets/models/testchar_contact.png` | contact-sheet destination |
| `--beauty PATH` | `assets/models/testchar_beauty.png` | beauty-sheet destination |
| `--app-preview PATH` | `assets/models/testchar_app.png` | runtime-emulation strip destination |
| `--expressions PATH` | off | also render the 4-panel shape-key proof |
| `--tex-size N` | `256` | atlas edge, multiple of 8, ≤ 256 |
| `--no-contact` / `--no-beauty` / `--no-app-preview` | off | skip that render |
| `--save-blend PATH` | off | also save the generated `.blend` |

The script self-verifies with `verify_glb.py` and exits non-zero if any contract
feature is missing.

### How the character is built (all procedural, no external assets)

* **Silhouette** — ~3.4 heads tall chibi. Head is a 7-ring lathe (`HEAD_RINGS`)
  closed by a single-vertex pole fan, so it stays smooth and round instead of
  reading as a block. Torso is a 7-ring loft, skirt a 4-ring flared shell with a
  pleated silhouette, arms/legs are tapered `sweep()` tubes with n-gon caps and a
  single-vertex tip fan at the hands/feet.
* **Hair** — a 24-column shell (`build_hair_cap`) over the skull with a *sawtooth
  fringe* hairline (`HC_TOOTH`) so the hairline reads as strands, plus two long
  side locks and an ahoge built with `sweep()`; the crown, nape and fringe get
  separate atlas regions so the colour breaks prevent a "helmet" look.
* **Face** — two 16-sided eye fans (white → iris → highlight rings), thin brow
  strips, a mouth, and blush, all as real geometry a few millimetres in front of
  the head shell, weighted to `head`.
* **Outfit** — cream blouse, navy sailor yoke with a subdivided V-neckerchief and
  a coral bow (built from `_front_quad`/`_front_tri` decals lifted 8 mm off the
  chest), navy pleated skirt, white knee socks with coral trim, coral shoes.
* **Atlas** — one 256×256 palette atlas (`REGIONS`), painted at 3× (768²) and
  box-downsampled so the diagetic details are anti-aliased before the device's
  128 px decode. Every region has its own painter (`paint_face`, `paint_eye`,
  `paint_hair_top`, …) returning sRGB tuples.
* **Rig / weights** — 17-bone humanoid armature; weights are distance-to-bone-
  segment over a per-part allow-list (`PART_BONES`), so no part ever binds to a
  bone it should not follow.
* **Expressions** — `blink`, `happy`, `surprised` shape keys, POSITION-only dense
  deltas, all values zeroed (Blender 5.2 `shape_key_add()` defaults to 1.0).

---

## 2. Verify a GLB (no Blender required)

```powershell
python tools/blender/verify_glb.py assets/models/testchar.glb `
    --expect-morphs blink,happy,surprised
```

Prints the GLB header/chunks, primitive attribute table, skin + joint names,
influence histogram, embedded texture dimensions, morph targets (count, names,
delta counts), humanoid-bone coverage and a PASS/FAIL. Exit code 0 = pass.
`--expect-morphs` makes it fail unless `extras.targetNames` matches that exact
order. It accepts several paths at once and is imported by the generators, so
there is exactly one implementation of the audit.

Real output for the shipped asset:

```
==============================================================================
GLB AUDIT  assets\models\testchar.glb
==============================================================================
container   : magic='glTF' version=2 declared=136452 actual=136452 bytes  OK
chunks      : JSON(4772), BIN(131652)
counts      : meshes=1 primitives=1 vertices=931 triangles=1310 accessors=10 skins=1 images=1 nodes=19 morph_targets=3
bbox        : min=(-0.759, 0.000, -0.232) max=(0.759, 1.628, 0.218)
------------------------------------------------------------------------------
primitives
  mesh 0 prim 0 : 1310 tris   931 verts  mode=4  attrs=JOINTS_0,NORMAL,POSITION,TEXCOORD_0,WEIGHTS_0
------------------------------------------------------------------------------
skins
  skin 0 : 17 joints, inverseBindMatrices accessor=9 type=MAT4 count=17
           joints: hips, leftUpperLeg, leftLowerLeg, leftFoot, rightUpperLeg, rightLowerLeg, rightFoot, spine, chest, leftUpperArm, leftLowerArm, leftHand, neck, head, rightUpperArm, rightLowerArm, rightHand
  max JOINTS_0 index=16  max influences/vertex=4  histogram={'1': 404, '2': 150, '3': 126, '4': 251}
------------------------------------------------------------------------------
textures
  image 0 : mimeType=image/png embedded=True size=256x256 bytes=40774 name='testchar_basecolor'
------------------------------------------------------------------------------
morph targets
  count=3  extras.targetNames=['blink', 'happy', 'surprised']  mesh.weights=[0, 0, 0]
  target 0 blink      attrs=POSITION  POSITION accessor=6 count=931  non-zero deltas=48  max_delta=0.06660
  target 1 happy      attrs=POSITION  POSITION accessor=7 count=931  non-zero deltas=66  max_delta=0.04640
  target 2 surprised  attrs=POSITION  accessor=8 count=931  non-zero deltas=64  max_delta=0.01932
------------------------------------------------------------------------------
humanoid bones : 17/17 present
nodes          : leftFoot, leftLowerLeg, leftUpperLeg, rightFoot, rightLowerLeg, rightUpperLeg, leftHand, leftLowerArm, leftUpperArm, head, neck, rightHand, rightLowerArm, rightUpperArm, chest, spine, hips, TestCharBody, TestCharRig
------------------------------------------------------------------------------
RESULT  : PASS
==============================================================================
```

---

## 3. Normalise any VRM / GLB into a spawnable GLB

```powershell
& $bl -b --factory-startup --python-exit-code 1 `
    --python tools/blender/vrm_to_kimios.py -- `
    --input "C:\path\to\avatar.vrm" --out assets/models/avatar.glb
```

The device loader (`user/kimi-gui/src/k3d`) has fixed-size arenas and a narrow
format contract; a stock VRoid/VRM avatar violates several at once (arbitrary bone
names, 40 k+ vertices, 57 morph targets, 18 textures, 137 joints, MToon-only
materials, unknown resting orientation). This tool rewrites the asset so it fits:

| what | how |
|---|---|
| **Skeleton** | rebuilt as exactly the 17 KimiOS bones. Source bones come from the VRM humanoid extension (`extensions.VRM.humanoid` for 0.x, `extensions.VRMC_vrm.humanoid` for 1.0) or name heuristics for plain glTF/FBX. Fingers, hair, accessories are folded into their nearest mapped ancestor and the weights summed. |
| **Skin weights** | re-targeted per canonical bone, top-4 influences, renormalised; vertices with no mapped influence fall back to the nearest bone segment. |
| **Meshes** | split glTF vertices merged by distance, all skinned meshes joined into one mesh, one UV layer. |
| **Transforms** | source matrices baked into vertices and bones; feet at Y=0, hips on X=0, identity root, character faces **+Z**. |
| **Facing** | decided from geometry (eye bones in front of the head, else toe bones in front of the foot), not from the spec version — real exports get the axis wrong. `--facing front/back` overrides. |
| **Triangles / vertices** | decimated (Collapse) into the budgets; the facial morph deltas are transferred onto the reduced topology through a KD-tree so expressions keep working. |
| **Textures** | every image re-encoded to PNG ≤ 256 px (the loader skips images > 384 px). Only four texture slots exist on the device: one texture per visual area is kept (eyes → face → cloth → hair → skin) and the rest are flattened to their *UV-covered* average colour (not the canvas average — character textures are mostly unused white), multiplied by the VRM 0.x MToon `_Color` tint. Near-uniform textures are flattened first, for free. |
| **Expressions** | VRM `blink`/`blink_l`/`blink_r`, `joy`/`fun`, `surprised` preset binds are combined into the three dense POSITION morphs `blink`, `happy`, `surprised`, all weights zeroed. |
| **Audit** | the exported GLB is parsed back with `verify_glb.py` and checked against the loader caps; `--report` writes a JSON summary and a non-zero exit means the result is not spawnable. |

Options:

| option | default | meaning |
|---|---|---|
| `--input PATH` | — | source `.vrm` / `.glb` / `.gltf` / `.fbx` (required) |
| `--out PATH` | `assets/models/<stem>.glb` | destination GLB |
| `--report PATH` | off | JSON report (source info, bone mapping, caps, warnings) |
| `--check-only` | off | audit the source against the caps without exporting |
| `--tex-size N` | `256` | max texture edge (≤ 256) |
| `--max-tris N` | `4000` | triangle budget |
| `--max-verts N` | `5000` | vertex budget (further clamped so `verts × morphs ≤ 16384`) |
| `--drop-morphs` | off | skip facial morphs to spend the whole budget on shape |
| `--no-decimate` | off | never decimate; warn instead |
| `--facing auto/front/back` | `auto` | override the front-axis detection |
| `--strict` | off | also enforce the full testchar contract (1 material, ≤ 4000 tris, ≤ 1000 verts, 3 morphs) |
| `--preview PATH` | off | render the *exported* GLB from the KimiOS front |
| `--save-blend PATH` | off | keep the intermediate `.blend` |

Verified on real avatars (all `RESULT: PASS`, loader caps satisfied):

| source | in | out |
|---|---|---|
| `fem_vroid.vrm` (VRM 0.x, VRoid Studio) | 19,352 tris / 42,599 verts / 59 joints / 18 images / 9 materials | 3,979 tris / 2,697 verts / 17 joints / 4 images, `blink,happy,surprised` |
| `masc_vroid.vrm` (VRM 0.x, VRoid Studio) | 20 k+ tris / 43 k verts | decimated to the same caps, PASS |
| `VRM1_Constraint_Twist_Sample.vrm` (VRM 1.0) | 3 meshes, 137 joints, 19 images | 3,988 tris / 3,046 verts / 17 joints / 4 images, PASS |
| `Avatar_Orion.vrm` (VRM 0.x, non-VRoid rig) | 6,200 tris / 22,480 verts / 65 joints | 3,9xx tris, 17 joints, PASS |
| `fixture_messy.vrm` (synthetic stress fixture) | `J_Bip_*` bone names, finger/hair bones, 2 meshes, 1024² textures, 5 k tris | 4,000 tris / 2,329 verts / 1 texture, PASS |
| `testchar.glb` (round trip) | our own asset | 3,979 tris / 955 verts, morphs preserved, PASS |

The synthetic fixture lives in `tools/blender/_work/` (git-ignored) and is
generated by `_work/make_vrm_fixture.py`. It carries a genuine
`extensions.VRM.humanoid` + `blendShapeMaster` payload with arbitrary node names
so the VRM-extension code path is actually exercised, not just the name heuristic.

---

## 4. Bake a VRM / character into a 4×4 sprite sheet

```powershell
& $bl -b --factory-startup --python-exit-code 1 `
    --python tools/blender/bake_vrm.py -- `
    --input "C:\path\to\avatar.vrm" --outdir assets/models/pet_frames
```

Writes `frame_00.png` … `frame_15.png` (96×96 cells, row-major) and
`sheet_4x4.png` (384×384), prints every cell and a JSON `BAKE|SUMMARY`.

* **Import** — `.vrm`, `.glb`, `.gltf` and `.fbx`. A `.vrm` is a glTF 2.0
  container, so it is copied to a temporary `.glb` because the importer keys off
  the extension.
* **Camera** — orthographic, fitted to the union of all baked poses with 12 %
  margin, so a raised arm never clips.
* **Poses** — samples the model's animation if it has one, otherwise a 16-step
  turntable.
* **Never a stub** — without `--input` it auto-selects the first model it finds,
  and if the repo has none it builds a coloured fallback prop, so the tool always
  produces output.

| option | default | meaning |
|---|---|---|
| `--input PATH` | auto-detect | source model |
| `--outdir DIR` | `assets/models/pet_frames` | frames + sheet destination |
| `--cell N` | `96` | cell edge in px |
| `--grid N` | `4` | cells per side |
| `--margin F` | `1.12` | camera padding factor |
| `--angle DEG` | `0` | camera yaw (VRM 0.x avatars can face away — try `180`) |
| `--opaque` | off | render on a grey backdrop instead of alpha |
| `--no-sheet` | off | write only the individual frames |

---

## What `testchar.glb` contains

* **Container** — GLB v2, one JSON chunk (4,772 B) + one BIN chunk (131,652 B),
  one self-contained buffer, no external URIs, no `extensionsUsed` (no Draco /
  meshopt decoder needed).
* **Mesh** — one primitive, `mode` 4 (TRIANGLES), **1,310 triangles**, 931
  vertices after UV/normal splitting, one UV layer. Attributes `POSITION`,
  `NORMAL`, `TEXCOORD_0`, `JOINTS_0`, `WEIGHTS_0` + `indices`.
* **Morph targets** — exactly 3, in order, with
  `meshes[0].extras.targetNames = ['blink','happy','surprised']`. Every target is
  `{ "POSITION": accessor }` — dense `VEC3 / FLOAT`, 931 deltas each, no `NORMAL`
  targets, no sparse accessors. Blink moves 48 vertices (max 66.6 mm), happy 66
  (46.4 mm), surprised 64 (19.3 mm). `meshes[0].weights = [0,0,0]`.
* **Skin** — one skin, 17 joints, `inverseBindMatrices` MAT4 × 17.
  `JOINTS_0` `VEC4 / UNSIGNED_BYTE`, `WEIGHTS_0` `VEC4 / FLOAT` normalised to 1,
  up to 4 influences per vertex (251 verts use 4, 126 use 3, 150 use 2, 404 use 1).
* **Armature** — the 17 bones survive with their exact names:
  `hips, spine, chest, neck, head, leftUpperArm, leftLowerArm, leftHand,
  rightUpperArm, rightLowerArm, rightHand, leftUpperLeg, leftLowerLeg, leftFoot,
  rightUpperLeg, rightLowerLeg, rightFoot`. Hierarchy:
  `hips → spine → chest → {neck → head, arms}` and `hips → legs`.
* **Texture** — one embedded 256×256 PNG (40,774 B) referenced by a
  `baseColorTexture`; the UV layout is a palette atlas (face, hair, trim, skin,
  cloth regions).
* **Material** — one `pbrMetallicRoughness` material, `metallicFactor` 0,
  `roughnessFactor` 0.8, `doubleSided: true`.
* **Orientation** — +Y up, character faces +Z, metres, feet at y = 0, bbox
  `(-0.759, 0, -0.232) … (0.759, 1.628, 0.218)`. Rest pose is a T-pose; no animations.

### Runtime-loader contract notes

1. `skin.joints` is a 17-entry node-index list in export order — **`JOINTS_0`
   values index that array, not the bone definition order** (`hips` is joint 0,
   `leftUpperLeg` joint 1, …). Resolve bones by `nodes[skin.joints[i]].name`.
2. Joint indices are `UNSIGNED_BYTE`; a loader that only reads `UNSIGNED_SHORT`
   joint buffers must accept 5121 as well.
3. `skins[0].skeleton` is absent (optional in glTF) — do not require it.
4. The scene root is node `TestCharRig`, whose children are `TestCharBody`
   (`mesh 0`, `skin 0`) and `hips`. Bone nodes carry TRS only.
5. `inverseBindMatrices` are column-major MAT4 per glTF 2.0.
6. Texture sampler is `magFilter 9729 / minFilter 9987`; all UVs stay inside their
   atlas patch, so mipmaps only soften edges.
7. `POSITION`/`NORMAL`/`TEXCOORD_0` are FLOAT and non-interleaved; no sparse data.
8. **Morphs**: `extras.targetNames` sits on the *mesh*, in the same order as
   `primitives[0].targets`. Evaluate `pos = base + Σ weight_i·delta_i` in mesh
   space *before* skinning. Mesh-level `weights` default to `[0,0,0]`.
9. `NORMAL` deltas are deliberately absent (`export_morph_normal` pinned off).

### Runtime framing (why the T-pose is so wide)

The pet renderer fits the **rest-pose** bounding sphere: centre = bbox centre,
`extent = max(0.5, half-diagonal)`, camera at `extent·2.2` with a 0.62 rad vertical
FOV. A narrow chibi silhouette therefore gets cropped. The arm span here
(`±0.759`) is deliberately wide so `extent` is dominated by height and the crop is
~2 cm; every posed frame hangs the arms down, so this is invisible in-app.

The renderer also downsamples every texture to `128×128` and samples with nearest
neighbouring, so the atlas is painted in bold, ring-aligned shapes rather than
fine detail.

---

## Blender 5.2 surprises worth knowing (found empirically)

Probe first — 5.2 is newer than almost all documentation. What actually tripped us
up:

* **Render engine is `BLENDER_EEVEE`.** The 4.2–4.x identifier
  `BLENDER_EEVEE_NEXT` is gone under `--factory-startup`.
* **The glTF exporter has no `PNG` image-format option.**
  `export_image_format` is `['AUTO','JPEG','WEBP','NONE']`; `AUTO` preserves the
  source PNG, which is what guarantees an embedded `image/png`.
* **`export_format` looks empty when introspected** (dynamic enum) but `'GLB'`
  works.
* **`shape_key_add()` creates keys at `value = 1.0`** (older Blender defaulted to
  0.0). Left alone, every expression exports as active and the glTF carries
  `weights = [1,1,1]`.
* **Morph exporter defaults are not loader-friendly.** `export_morph_normal`
  defaults to `True` and `export_try_sparse_sk` defaults to `True`, which emits
  **sparse accessors** for facial expressions. Both are pinned off.
* **A modifier cannot be applied to a mesh with shape keys.** Decimation therefore
  snapshots the keys, deletes them, applies the modifier and rebuilds the keys by
  nearest-vertex transfer (KD-tree).
* **`Image.scale()` does not change what the exporter embeds.** A packed glTF image
  keeps its original source bytes; `pack()` alone re-packs the old file. The only
  reliable downscale is `save()` a real PNG, load it as a new datablock and
  repoint the texture nodes.
* **VRM 0.x colour lives in MToon, not in glTF.** `materialProperties[].vectorProperties._Color`
  holds the real colour (keyed by material *name*, not index); the glTF
  `pbrMetallicRoughness` factor is usually white. Ignoring it makes converted VRoid
  faces flat white.
* **VRM front axis is unreliable.** The spec disagrees between 0.x and 1.0 and real
  exports get it wrong; the eye bones (in front of the head) and toe bones are the
  geometric ground truth. Bone *tails* are useless because glTF has no tails and
  the importer synthesises them.
* **The glTF importer adds an `Icosphere` bone-display mesh** unless you pass
  `disable_bone_shape=True`; it lives in the `glTF_not_exported` collection but is
  *not* hidden from render, so it inflates bounding boxes and renders as a spiky
  ball. Both scripts pass the flag *and* filter such objects defensively. Without
  `--factory-startup`, the default scene Cube leaks into imports the same way —
  always clear the scene before importing.
* **`Object.vertex_groups` is not on `bpy.types.Object`**: a
  `hasattr(bpy.types.Object, "vertex_groups")` probe returns `False` while real
  instances have it. Probe instances, not RNA classes.
* **PowerShell text tools corrupt UTF-8.** `Get-Content | Set-Content` re-encoded a
  script to the system code page and destroyed non-ASCII literals; use the editor
  tools (or explicit `-Encoding utf8`) for files with non-ASCII text.
* Exit codes: with `--python-exit-code 1`, a raising script makes Blender exit 1.
  Beware piping through `Select-String`/`grep` in PowerShell — check
  `$LASTEXITCODE` from a direct invocation.

---

## Customising

* **Poses** — `POSES` in `make_test_character.py`: `{bone_name: (rx, ry, rz)}` in
  **world axes**, converted into bone space by `rotate_world()`. Labels are the dict
  keys; the contact sheet is a single render of duplicated rigs, so adding columns
  only needs spacing/camera tweaks (`CONTACT_SPACING`, `CONTACT_ORTHO`).
* **Rig** — `BONES` (name, parent, head, tail, metres, Blender Z-up).
* **Body** — `build_geometry()`: `HEAD_RINGS` lathe, torso loft, `sweep()` limb
  tubes. Weights come from `PART_BONES` (which bones a part may bind to) plus
  distance-to-segment, so new parts only need an allow-list entry.
* **Hair** — `build_hair_cap()` (`HC_*` constants: radii, hairline heights, tooth
  size) and `build_hair_locks()`. The fringe is part of the cap, drawn by the
  sawtooth `hairline_z()`.
* **Texture** — `REGIONS` + the `paint_*` painters in `make_atlas()`; all drawing
  uses normalised region coordinates, so `--tex-size 128` also works.
* **Shape keys** — `EXPRESSIONS` + `add_expression_shape_keys()`; deltas are
  computed from basis positions. Adding an expression means appending the name,
  adding a delta block and leaving its value at 0.
* **Budgets** — the verifier fails the build if triangles ≥ 4000 or any texture
  edge > 256 px, so regressions cannot slip through silently.

---

## Files

| file | purpose |
|---|---|
| `make_test_character.py` | procedural chibi character: rig, mesh, atlas, shape keys; exports the GLB and renders contact/beauty/app/expression sheets |
| `vrm_to_kimios.py` | any VRM/glTF/GLB/FBX → canonical 17-bone, caps-compliant, PNG-textured GLB; audits the result and can render a preview |
| `verify_glb.py` | standalone (or imported) GLB container/accessor/skin/texture/morph audit |
| `bake_vrm.py` | VRM/glTF/FBX → 16 × 96 px 4×4 sprite pack, camera fitted to all poses |
| `README.md` | this file |

Scratch output (generated atlases, temporary fixtures, downloaded sample VRMs,
probe scripts) goes to `tools/blender/_work/`, which is git-ignored.