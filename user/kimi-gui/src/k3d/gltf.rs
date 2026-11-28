//! glTF 2.0 / VRM 1.0 loader.
//!
//! Streams a `.glb`/`.vrm` through [`ReadAt`] instead of holding it in RAM: the
//! container header, then the JSON chunk, then random-access reads of only the
//! accessors the model actually needs. That matters because a real VRM is
//! ~10 MB of binary buffer, far beyond the 4 MiB user heap.
//!
//! Scope: POSITION/NORMAL/TEXCOORD_0, JOINTS_0/WEIGHTS_0, indices, nodes with
//! TRS or matrix, skins, materials (base colour + base colour texture + alpha
//! mode), embedded PNG images, and the `VRMC_vrm` humanoid bone map.
//!
//! NOT yet implemented (tracked in the 3D plan): sparse accessors, morph
//! targets/expressions, spring bones, MToon materials, JPEG images.

use super::model::{LoadError, ModelArena, ReadAt};

/// Parse a GLB from `read` into `arena`.
pub fn load(read: &mut dyn ReadAt, size: u64, arena: &mut ModelArena) -> Result<(), LoadError> {
    let _ = (read, size, arena);
    Err(LoadError::Unsupported)
}