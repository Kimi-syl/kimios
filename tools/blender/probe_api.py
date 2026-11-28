"""Probe Blender 5.2 API surface before relying on it. Prints, never raises."""
import bpy, sys, json

def line(t):
    print("PROBE|" + t)

line("bpy_version=%r" % (bpy.app.version,))
line("version_string=%s" % bpy.app.version_string)
line("python=%s" % sys.version.replace("\n", " "))

# --- render engines ---
try:
    items = bpy.types.RenderSettings.bl_rna.properties["engine"].enum_items
    line("engines=%r" % [e.identifier for e in items])
except Exception as e:
    line("engines_ERROR=%r" % (e,))

# --- gltf export operator properties ---
try:
    props = bpy.ops.export_scene.gltf.get_rna_type().properties
    names = sorted(p.identifier for p in props)
    line("gltf_export_props=%r" % names)
except Exception as e:
    line("gltf_export_ERROR=%r" % (e,))

# --- gltf/other import operators ---
import_ops = [n for n in dir(bpy.ops.import_scene) if not n.startswith("_")]
line("import_scene_ops=%r" % sorted(import_ops))

# --- image formats ---
try:
    fmt = bpy.types.ImageFormatSettings.bl_rna.properties["file_format"].enum_items
    line("image_file_formats=%r" % [e.identifier for e in fmt])
except Exception as e:
    line("imgfmt_ERROR=%r" % (e,))

# --- Principled BSDF socket names ---
bpy.ops.wm.read_factory_settings(use_empty=True)
mat = bpy.data.materials.new("probe")
mat.use_nodes = True
bsdf = mat.node_tree.nodes.get("Principled BSDF")
line("has_principled=%r" % (bsdf is not None,))
if bsdf:
    line("principled_inputs=%r" % [s.name for s in bsdf.inputs])

# --- mesh api basics: custom normals / color attributes ---
me = bpy.data.meshes.new("m")
line("mesh_has_shade_smooth=%r" % hasattr(me, "shade_smooth"))
line("mesh_has_polygons_lookup=%r" % hasattr(me, "polygons"))
line("mesh_has_corner_normals=%r" % hasattr(me, "corner_normals"))

# --- armature / vertex group ---
line("obj_has_vertex_groups=%r" % hasattr(bpy.types.Object, "vertex_groups"))
line("mod_armature=%r" % hasattr(bpy.types, "ArmatureModifier"))

# --- render output settings ---
r = bpy.context.scene.render
line("render_res=%r" % (tuple(r.resolution_x for _ in [0]) + (bpy.context.scene.render.resolution_y,)))
line("filepath_attr=%r" % hasattr(r, "filepath"))
line("image_settings=%r" % (bpy.context.scene.render.image_settings.file_format,))
line("film_transparent=%r" % hasattr(r, "film_transparent"))
line("render_has_engine=%r" % hasattr(r, "engine"))

# --- world / lighting ---
line("world_available=%r" % (bpy.data.worlds is not None))
line("scene_eevee=%r" % ("eevee" in dir(bpy.context.scene)))
line("scene_cycles=%r" % ("cycles" in dir(bpy.context.scene)))

# --- material slots / uv layers ---
line("mesh_uv_layers=%r" % hasattr(me, "uv_layers"))
line("mesh_loops=%r" % hasattr(me, "loops"))

# --- keyframe / pose ---
line("armature_has_pose=%r" % hasattr(bpy.types.Armature, "pose"))

print("PROBE_DONE")