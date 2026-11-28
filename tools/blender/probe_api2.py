"""Probe 2: enums, object/mesh API, armature, image embedding."""
import bpy, sys

def line(t):
    print("PROBE|" + t)

line("version=%s" % bpy.app.version_string)

def enum_of(typ, prop):
    try:
        return [e.identifier for e in typ.bl_rna.properties[prop].enum_items]
    except Exception as e:
        return "ERR:%r" % (e,)

line("export_format_enum=%r" % enum_of(bpy.ops.export_scene.gltf.get_rna_type(), "export_format"))
line("export_image_format_enum=%r" % enum_of(bpy.ops.export_scene.gltf.get_rna_type(), "export_image_format"))
line("export_animation_mode_enum=%r" % enum_of(bpy.ops.export_scene.gltf.get_rna_type(), "export_animation_mode"))
line("export_influence_nb=%r" % enum_of(bpy.ops.export_scene.gltf.get_rna_type(), "export_influence_nb"))
line("export_materials_enum=%r" % enum_of(bpy.ops.export_scene.gltf.get_rna_type(), "export_materials"))

line("import_gltf_props=%r" % sorted(p.identifier for p in bpy.ops.import_scene.gltf.get_rna_type().properties))

bpy.ops.wm.read_factory_settings(use_empty=True)
# --- object instance API ---
me = bpy.data.meshes.new("m")
me.from_pydata([(0,0,0),(1,0,0),(0,1,0)], [], [(0,1,2)])
ob = bpy.data.objects.new("o", me)
bpy.context.scene.collection.objects.link(ob)
line("inst_has_vertex_groups=%r" % hasattr(ob, "vertex_groups"))
line("inst_vg_add=%r" % hasattr(ob.vertex_groups, "new") if hasattr(ob, "vertex_groups") else "NOATTR")
line("mesh_uv_layers_new=%r" % hasattr(me.uv_layers, "new"))
line("mesh_validate=%r" % hasattr(me, "validate"))
line("mesh_normals_split_custom=%r" % hasattr(me, "normals_split_custom_set"))
line("obj_modifiers_new=%r" % hasattr(ob.modifiers, "new"))
line("poly_use_smooth=%r" % hasattr(me.polygons[0], "use_smooth"))
line("mesh_shade_smooth_attr=%r" % hasattr(me, "shade_smooth"))

# --- armature creation path ---
arm = bpy.data.armatures.new("A")
aob = bpy.data.objects.new("Arm", arm)
bpy.context.scene.collection.objects.link(aob)
bpy.context.view_layer.objects.active = aob
bpy.ops.object.mode_set(mode='EDIT')
eb = arm.edit_bones.new("hips")
eb.head = (0,0,0); eb.tail = (0,0,1)
line("edit_bone_ok=%r" % (arm.edit_bones["hips"].name,))
line("edit_bone_roll_attr=%r" % hasattr(eb, "roll"))
bpy.ops.object.mode_set(mode='OBJECT')
line("pose_bones=%r" % list(arm.bones.keys()))
line("bone_use_deform=%r" % arm.bones["hips"].use_deform)
line("object_has_pose_bones=%r" % hasattr(aob, "pose"))
line("armature_rest_ok=%r" % (arm.edit_bones is not None))

# --- render settings / eevee ---
sc = bpy.context.scene
sc.render.engine = 'BLENDER_EEVEE'
line("engine_set=%r" % sc.render.engine)
line("eevee_engine_accepts=%r" % enum_of(bpy.types.RenderSettings, "engine"))
line("scene_eevee_attrs=%r" % sorted(a for a in dir(sc.eevee) if not a.startswith("_"))[:40])
line("img_format_png=%r" % ("PNG" in enum_of(bpy.types.ImageFormatSettings, "file_format")))
line("render_filepath=%r" % sc.render.filepath)

# --- image creation / save ---
img = bpy.data.images.new("tex", 8, 8, alpha=True)
line("image_new=%r" % (img.size[:],))
line("image_has_pixels=%r" % (len(img.pixels) == 8*8*4,))
line("image_file_format_attr=%r" % hasattr(img, "file_format"))

# --- world nodes ---
w = bpy.data.worlds.new("W")
line("world_has_nodes=%r" % hasattr(w, "node_tree"))
line("mat_use_nodes_deprecated_warning=%r" % "use_nodes" in bpy.types.Material.bl_rna.properties)
print("PROBE_DONE")