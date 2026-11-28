# Blender asset pipeline (KimiOS)

Headless Blender scripts that produce the device-ready 3D test character for the
`no_std` software rasteriser, plus a VRM → sprite-sheet baker for the pet pack.

Everything here is driven by Blender's Python API from the command line. There is
**no Blender MCP server** in this environment and none is needed.

---

## Environment (verified)

| item | value |
|---|---|
| Blender | **5.2.3 LTS** (hash `e13b728c6880`, built 2026-10-22), bundle Python 3.13.13 |
| Blender path | `C:\Program Files\Blender Foundation\Blender 5.2\blender.exe` — **not on PATH** |
| System Python | 3.14.8 (used for `verify_glb.py`; it needs no `bpy`) |
| Host | Windows, headless (`-b`); EEVEE renders fine without a GPU display |

The canonical invocation — `--factory-startup` guarantees no user add-ons,
`--python-exit-code 1` turns an uncaught script exception into a non-zero exit:

```powershell
$bl = "C:\Program Files\Blender Foundation\Blender 5.2\blender.exe"
& $bl -b --factory-startup --python-exit-code 1 --python <script.py> -- <script args>
```

`-- <args>` is optional; arguments after `--` are passed to the script, not Blender.

---

## 1. Generate the test character

```powershell
& $bl -b --factory-startup --python-exit-code 1 `
    --python tools/blender/make_test_character.py
```

Outputs, relative to the repo root:

| file | what |
|---|---|
| `assets/models/testchar.glb` | 53,776-byte skinned GLB (see contract below) |
| `assets/models/testchar_contact.png` | 1920×600 four-pose proof render (T-pose / wave / walk / kick) |

The character is built entirely from code — no external model, no add-on, no MCP:
a lofted torso, tapered limb tubes, box head/hands/feet, a hand-drawn 256×256
palette atlas (face on the front of the head, cyan diamond on the chest), a
17-bone humanoid armature and distance-to-bone-segment skin weights.

Options (all after `--`):

| option | default | meaning |
|---|---|---|
| `--out PATH` | `<repo>/assets/models/testchar.glb` | GLB destination |
| `--contact PATH` | `<repo>/assets/models/testchar_contact.png` | contact-sheet destination |
| `--tex-size N` | `256` | atlas size, multiple of 8, ≤ 256 (128 and 256 tested) |
| `--no-contact` | off | skip the render |
| `--save-blend PATH` | off | also save the generated `.blend` for inspection |

The script self-verifies: it parses the GLB it just wrote (via `verify_glb.py`)
and exits non-zero if any contract feature is missing.

---

## 2. Verify a GLB (no Blender required)

```powershell
python tools/blender/verify_glb.py assets/models/testchar.glb
```

Prints the GLB header/chunks, primitive attribute table, skin + joint names,
influence histogram, embedded texture dimensions, humanoid-bone coverage and a
PASS/FAIL. Exit code 0 = pass. It accepts several paths at once and is imported
by `make_test_character.py`, so there is exactly one implementation of the audit.

Real output for the shipped asset:

```
==============================================================================
GLB AUDIT  assets\models\testchar.glb
==============================================================================
container   : magic='glTF' version=2 declared=53776 actual=53776 bytes  OK
chunks      : JSON(3720), BIN(50028)
counts      : meshes=1 primitives=1 vertices=600 triangles=524 accessors=7 skins=1 images=1 nodes=19
bbox        : min=(-0.745, 0.000, -0.126) max=(0.745, 1.680, 0.185)
------------------------------------------------------------------------------
primitives
  mesh 0 prim 0 :  524 tris   600 verts  mode=4  attrs=JOINTS_0,NORMAL,POSITION,TEXCOORD_0,WEIGHTS_0
      JOINTS_0    accessor 3   VEC4   UNSIGNED_BYTE   count=600
      NORMAL      accessor 1   VEC3   FLOAT           count=600
      POSITION    accessor 0   VEC3   FLOAT           count=600
      TEXCOORD_0  accessor 2   VEC2   FLOAT           count=600
      WEIGHTS_0   accessor 4   VEC4   FLOAT           count=600
------------------------------------------------------------------------------
skins
  skin 0 : 17 joints, inverseBindMatrices accessor=6 type=MAT4 count=17
           joints: hips, leftUpperLeg, leftLowerLeg, leftFoot, rightUpperLeg, rightLowerLeg, rightFoot, spine, chest, leftUpperArm, leftLowerArm, leftHand, neck, head, rightUpperArm, rightLowerArm, rightHand
  max JOINTS_0 index=16  max influences/vertex=4  histogram={'2': 248, '3': 232, '4': 120}
------------------------------------------------------------------------------
textures
  image 0 : mimeType=image/png embedded=True size=256x256 bytes=14595 name='testchar_basecolor'
------------------------------------------------------------------------------
humanoid bones : 17/17 present
nodes          : leftFoot, leftLowerLeg, leftUpperLeg, rightFoot, rightLowerLeg, rightUpperLeg, leftHand, leftLowerArm, leftUpperArm, head, neck, rightHand, rightLowerArm, rightUpperArm, chest, spine, hips, TestCharBody, TestCharRig
------------------------------------------------------------------------------
RESULT  : PASS
==============================================================================
```

---

## 3. Bake a VRM / character into a 4×4 sprite sheet

```powershell
& $bl -b --factory-startup --python-exit-code 1 `
    --python tools/blender/bake_vrm.py -- `
    --input "C:\path\to\avatar.vrm" --outdir assets/models/pet_frames
```

It prints every cell it renders and a JSON `BAKE|SUMMARY`, and writes:

* `frame_00.png` … `frame_15.png` — 16 cells, 96×96 px (cell row-major)
* `sheet_4x4.png` — 384×384 combined sheet (skip with `--no-sheet`)

Behaviour:

* **Import** — `.vrm`, `.glb`, `.gltf` (glTF importer; a `.vrm` is a glTF 2.0
  container, so it is copied to a temporary `.glb` first because the importer
  keys off the extension) and `.fbx`.
* **Camera** — an orthographic camera is fitted to the **union of all 16 baked
  poses** (evaluated, modifier-deformed vertices in world space), then framed
  with 12 % margin. It never clips a raised arm or a wide turntable frame.
* **Poses** — if the imported model carries an animation, 16 frames are sampled
  evenly across its action range (`mode: animation`). If it has no animation —
  which is normal for a VRM — the model is turned through 360° in 16 steps
  (`mode: turntable`).
* **Never a stub** — with no `--input` it auto-selects the first model it finds
  (`assets/models/*.vrm`, then `assets/models/testchar.glb`); if the repo has no
  model at all it builds a coloured fallback prop, so the tool always runs end to
  end and prints what it produced.

| option | default | meaning |
|---|---|---|
| `--input PATH` | auto-detect | source `.vrm` / `.glb` / `.gltf` / `.fbx` |
| `--outdir DIR` | `<repo>/assets/models/pet_frames` | frames + sheet destination |
| `--cell N` | `96` | cell edge in px (4×4 sheet = 384×384) |
| `--grid N` | `4` | cells per side → `N*N` frames |
| `--margin F` | `1.12` | camera padding factor |
| `--angle DEG` | `0` | camera yaw; VRM 0.x avatars can face away — try `180` |
| `--opaque` | off | render on a grey backdrop instead of transparent (alpha) |
| `--no-sheet` | off | write only the individual frames |

---

## What `testchar.glb` contains

* **Container** — GLB v2, one JSON chunk (3,720 B) + one BIN chunk (50,028 B),
  one self-contained buffer (`byteLength` 50028, no external URIs, no
  `extensionsUsed`, so no Draco/meshopt decoder is required).
* **Mesh** — one primitive, `mode` 4 (TRIANGLES), **524 triangles / 1,572
  indices** (`UNSIGNED_SHORT`), 600 vertices after UV/normal splitting.
  Attributes `POSITION`, `NORMAL`, `TEXCOORD_0`, `JOINTS_0`, `WEIGHTS_0`,
  plus `indices`.
* **Skin** — one skin, 17 joints, `inverseBindMatrices` = MAT4 × 17.
  `JOINTS_0` is `VEC4 / UNSIGNED_BYTE`; `WEIGHTS_0` is `VEC4 / FLOAT`,
  normalised to 1.0, **up to 4 influences per vertex**
  (120 vertices use 4, 232 use 3, 248 use 2).
* **Armature** — the 17 standard humanoid bones survive export with their exact
  names:
  `hips, spine, chest, neck, head, leftUpperArm, leftLowerArm, leftHand,
  rightUpperArm, rightLowerArm, rightHand, leftUpperLeg, leftLowerLeg, leftFoot,
  rightUpperLeg, rightLowerLeg, rightFoot`.
  Hierarchy: `hips → spine → chest → {neck → head, left arm, right arm}` and
  `hips → legs`.
* **Texture** — a single 256×256 PNG (`image/png`) embedded in the BIN chunk,
  referenced by a `baseColorTexture`. The UV layout is a palette atlas; the front
  of the head maps to the drawn face, the shirt band is planar-mapped so the
  chest emblem shows.
* **Material** — one `pbrMetallicRoughness` material, `metallicFactor` 0,
  `roughnessFactor` 0.75, `doubleSided: true` (safe to draw without culling).
* **Orientation** — glTF convention: **+Y up, character faces +Z**, units are
  metres, feet at y = 0, bbox `(-0.745, 0, -0.126) … (0.745, 1.68, 0.185)`.
  Rest pose is the T-pose; there are **no animations** in this asset.

### Runtime-loader contract notes

1. `skin.joints` is a 17-entry node-index list in export order — **JOINTS_0
   values index that array, not the bone definition order** (`hips` is joint 0,
   `leftUpperLeg` joint 1, …). Resolve bones by `nodes[skin.joints[i]].name`.
2. Joint indices are `UNSIGNED_BYTE`; a loader that only reads `UNSIGNED_SHORT`
   joint buffers must accept 5121 as well.
3. `skins[0].skeleton` is absent (optional in glTF) — do not require it.
4. The scene root is node `TestCharRig`, whose children are node `TestCharBody`
   (`mesh 0`, `skin 0`) and node `hips`. Bone nodes carry TRS only.
5. `inverseBindMatrices` are column-major MAT4 per glTF 2.0.
6. Texture sampler is `magFilter 9729 / minFilter 9987` (LINEAR /
   LINEAR_MIPMAP_LINEAR) and wraps default to REPEAT; the atlas keeps all UVs
   inside their patch, so mipmaps only soften the edges.
7. `POSITION`/`NORMAL`/`TEXCOORD_0` are all FLOAT and non-interleaved; accessors
   have no sparse data.

---

## Blender 5.2 surprises worth knowing (found empirically, not from tutorials)

Probe first — 5.2 is newer than almost all documentation. What actually tripped
us up:

* **Render engine is `BLENDER_EEVEE`.** The 4.2–4.x identifier
  `BLENDER_EEVEE_NEXT` is gone; the enum is just `['BLENDER_EEVEE']` under
  `--factory-startup` (Cycles is not enabled).
* **The glTF exporter has no `PNG` image-format option.**
  `export_image_format` is `['AUTO', 'JPEG', 'WEBP', 'NONE']`. `AUTO` preserves
  the source PNG, so that is what we pass to guarantee an embedded `image/png`.
* **`export_format` looks empty when introspected**
  (`enum_items` returns `[]` because it is a dynamic enum) but `'GLB'` works.
* The exporter argument list is not the 3.x one: influences are controlled by
  `export_influence_nb` (int) + `export_all_influences`, and there are new
  switches used here: `export_def_bones`, `export_rest_position_armature`,
  `export_try_sparse_sk`, `export_optimize_animation_size`, ….
* **`Object.vertex_groups` is not on `bpy.types.Object`** — a
  `hasattr(bpy.types.Object, "vertex_groups")` capability probe returns `False`,
  while a real object instance has it. Probe instances, not RNA classes.
* **The glTF importer adds an `Icosphere` bone-display mesh** (one tiny
  icosphere object shared as `custom_shape` by every bone) unless you pass
  `disable_bone_shape=True`. It lives in the `glTF_not_exported` collection but
  is *not* hidden from render, so it silently inflates Union-BBox camera fits
  and renders as a spiky ball. `bake_vrm.py` passes the flag *and* filters such
  objects defensively.
* `Material.use_nodes` is deprecated (warning: removal in Blender 6.0) but still
  required to build node materials.
* Blender 5.x uses **slotted actions**; `pose_bone.keyframe_insert()` still
  creates the action and slot automatically, and `action.frame_range` still
  reports the range used by the animation baker.
* Exit codes: with `--python-exit-code 1`, a script that raises (including
  `SystemExit(1)`) makes Blender exit 1; a clean script exits 0. Beware piping
  the process through `Select-String`/`grep` in PowerShell — check
  `$LASTEXITCODE` from a direct invocation instead.
* `--factory-startup` matters for reproducibility: no user add-ons (there is no
  VRM add-on installed either, so `bake_vrm.py` imports VRM through the bundled
  glTF importer, which reads the glTF 2.0 payload and ignores VRM/MToon
  extensions).

---

## Customising

* **Poses** — `POSES` in `make_test_character.py`: `{bone_name: (rx, ry, rz)}` in
  **world axes**, converted into bone space by `rotate_world()`. Labels are the
  dict keys. Add rows to render more poses (the contact sheet is a single render
  of duplicated rigs, so any number of columns works with spacing/camera tweaks).
* **Rig** — `BONES` (name, parent, head, tail, all in metres, Blender Z-up).
* **Body** — `build_geometry()`: torso loft rings, limb tubes, boxes. Weights come
  from `PART_BONES` (which bones a part may bind to) + distance-to-segment, so
  new parts only need an allow-list entry.
* **Texture** — `PATCHES` (8×8 grid of sRGB colours) and `make_atlas()`; all
  drawing uses normalised patch coordinates, so `--tex-size 128` works too.
* **Budgets** — the verifier fails the build if triangles ≥ 4000 or any texture
  edge > 256 px, so regressions cannot slip through silently.

---

## Files

| file | purpose |
|---|---|
| `make_test_character.py` | procedural rig + mesh + atlas; exports GLB and renders the contact sheet |
| `verify_glb.py` | standalone (or imported) GLB container/accessor/skin/texture audit |
| `bake_vrm.py` | VRM/glTF/FBX → 16 × 96 px 4×4 sprite pack, camera fitted to all poses |
| `README.md` | this file |

Scratch output (the generated atlas, temporary fixtures) goes to
`tools/blender/_work/`, which is git-ignored.