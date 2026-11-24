// Node 24 compatibility shim: OpenCode's Node bundle imports a few Node 26
// exports that are absent on the Node 24 mobile runtime.
export function isSea() { return false }
export function getAssetKeys() { return [] }
export function getRawAsset() { throw new Error("SEA assets unavailable") }
export function getAsset() { throw new Error("SEA assets unavailable") }
export function getAssetAsBlob() { throw new Error("SEA assets unavailable") }