import { register } from 'node:module';
register('./plugin-artifact-loader.mjs', import.meta.url, {
  data: { modules: process.env.K_CODER_ARTIFACT_NODE_MODULES },
});
