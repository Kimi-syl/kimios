"""Spike: minimal skinned mesh + embedded PNG -> GLB, then parse the GLB back."""
import bpy, json, struct, os, sys, math

OUT = os.path.join(os.getcwd(), "tools", "blender", "_work", "spike.glb")
os.makedirs(os.path.dirname(OUT), exist_ok=True)
bpy.ops.wm.read_factory_settings(use_empty=True)

# --- texture: 64x64, half red half blue, saved as PNG ---
img = bpy.data.images.new("spiketex", 64, 64, alpha=True)
px = [0.0] * (64 * 64 * 4)
for y in range(64):
    for x in range(64):
        i = (y * 64 + x) * 4
        if x < 32:
            px[i:i+4] = [0.8, 0.1, 0.1, 1.0]
        else:
            px[i:i+4] = [0.1, 0.2, 0.8, 1.0]
img.pixels.foreach_set(px)
texpath = os.path.join(os.path.dirname(OUT), "spiketex.png")
img.filepath_raw = texpath
img.file_format = 'PNG'
img.save()
print("SPIKE|saved texture", texpath, os.path.getsize(texpath))

# --- material ---
mat = bpy.data.materials.new("spikemat")
mat.use_nodes = True
nt = mat.node_tree
bsdf = nt.nodes["Principled BSDF"]
tex = nt.nodes.new("ShaderNodeTexImage")
tex.image = img
nt.links.new(tex.outputs["Color"], bsdf.inputs["Base Color"])

# --- armature: 2 bones ---
arm = bpy.data.armatures.new("SpikeArm")
aob = bpy.data.objects.new("SpikeRig", arm)
bpy.context.scene.collection.objects.link(aob)
bpy.context.view_layer.objects.active = aob
bpy.ops.object.mode_set(mode='EDIT')
b0 = arm.edit_bones.new("hips"); b0.head = (0, 0, 0.5); b0.tail = (0, 0, 1.0)
b1 = arm.edit_bones.new("head"); b1.head = (0, 0, 1.0); b1.tail = (0, 0, 1.4); b1.parent = b0; b1.use_connect = True
bpy.ops.object.mode_set(mode='OBJECT')

# --- mesh ---
me = bpy.data.meshes.new("SpikeMesh")
me.from_pydata([(-.3,-.3,.5),(.3,-.3,.5),(.3,.3,.5),(-.3,.3,.5),
                (-.3,-.3,1.2),(.3,-.3,1.2),(.3,.3,1.2),(-.3,.3,1.2)],
               [], [(0,1,2,3),(4,5,6,7),(0,1,5,4),(1,2,6,5),(2,3,7,6),(3,0,4,7)])
me.validate()
uv = me.uv_layers.new(name="UVMap")
for i, loop in enumerate(me.loops):
    v = me.vertices[loop.vertex_index].co
    uv.data[i].uv = (0.25 if v.x < 0 else 0.75, 0.5)
ob = bpy.data.objects.new("SpikeBody", me)
ob.data.materials.append(mat)
bpy.context.scene.collection.objects.link(ob)
for bone in ("hips", "head"):
    ob.vertex_groups.new(name=bone)
for v in me.vertices:
    g = "head" if v.co.z > 1.0 else "hips"
    ob.vertex_groups[g].add([v.index], 1.0, 'REPLACE')
mod = ob.modifiers.new("Armature", 'ARMATURE')
mod.object = aob
ob.parent = aob

bpy.ops.object.select_all(action='DESELECT')
print("SPIKE|exporting")
bpy.ops.export_scene.gltf(
    filepath=OUT, export_format='GLB', use_selection=False,
    export_apply=False, export_animations=False, export_skins=True,
    export_influence_nb=4, export_texcoords=True, export_normals=True,
    export_materials='EXPORT', export_image_format='AUTO',
    export_yup=True, export_extras=False, export_cameras=False, export_lights=False,
)
print("SPIKE|glb size", os.path.getsize(OUT))

# --- parse GLB ---
with open(OUT, "rb") as f:
    data = f.read()
magic, ver, length = struct.unpack("<4sII", data[:12])
print("SPIKE|magic", magic, "ver", ver, "len", length, "filesize", len(data))
off = 12
gltf = None
while off < length:
    clen, ctype = struct.unpack("<I4s", data[off:off+8])
    if ctype == b'JSON':
        gltf = json.loads(data[off+8:off+8+clen].decode("utf-8"))
    off += 8 + clen
print("SPIKE|meshes", len(gltf.get("meshes", [])), "skins", len(gltf.get("skins", [])), "accessors", len(gltf.get("accessors", [])), "images", len(gltf.get("images", [])))
for m in gltf["meshes"]:
    for p in m["primitives"]:
        print("SPIKE|prim attrs", sorted(p["attributes"].keys()), "mode", p.get("mode"), "idx_accessor", p.get("indices"))
for s in gltf.get("skins", []):
    print("SPIKE|skin joints", len(s["joints"]), "ibm_accessor", s.get("inverseBindMatrices"))
    names = [gltf["nodes"][j].get("name") for j in s["joints"]]
    print("SPIKE|skin joint names", names)
print("SPIKE|node names", [n.get("name") for n in gltf.get("nodes", [])])
for i, im in enumerate(gltf.get("images", [])):
    print("SPIKE|image", i, im.get("mimeType"), "bufferView", im.get("bufferView"), "uri", im.get("uri"))
for a in gltf["accessors"]:
    print("SPIKE|accessor", a.get("name"), a["type"], a["componentType"], a["count"])
print("SPIKE_DONE")