import { createRequire } from 'node:module';
import { join, isAbsolute } from 'node:path';
import { pathToFileURL } from 'node:url';

let artifactRequire;
export function initialize({ modules }) {
  if (!modules || !isAbsolute(modules)) throw new Error('Artifact runtime module directory is missing. Use plugin-artifact-runtime.ps1.');
  artifactRequire = createRequire(join(modules, '__k_coder_resolve__.cjs'));
}
export async function resolve(specifier, context, nextResolve) {
  if (specifier === '@oai/artifact-tool' || specifier.startsWith('@oai/artifact-tool/')) {
    return { url: pathToFileURL(artifactRequire.resolve(specifier)).href, shortCircuit: true };
  }
  return nextResolve(specifier, context);
}
