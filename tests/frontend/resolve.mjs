// Résolution des imports sans extension du frontend (Vite les accepte, Node non).
export async function resolve(specifier, context, next) {
  try {
    return await next(specifier, context);
  } catch (error) {
    const relative = specifier.startsWith("./") || specifier.startsWith("../");
    if (error?.code !== "ERR_MODULE_NOT_FOUND" || !relative || /\.[cm]?[jt]sx?$/.test(specifier)) throw error;
    for (const ext of [".ts", ".tsx"]) {
      try {
        return await next(specifier + ext, context);
      } catch {
        // extension suivante
      }
    }
    throw error;
  }
}
