export async function resolve(specifier, context, nextResolve) {
  if (specifier === "node:sea") {
    return { url: new URL("./sea-shim.mjs", import.meta.url).href, shortCircuit: true }
  }
  return nextResolve(specifier, context)
}