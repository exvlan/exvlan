import { defineConfig } from 'vite-plus/pack';
import pkg from './package.json' with { type: 'json' };

export default defineConfig({
  entry: ['guest-js/index.ts'],
  platform: 'node',
  deps: {
    neverBundle: Object.keys(pkg.devDependencies)
  },
  format: [ 'esm','cjs'],
  outDir: 'dist-js',
  clean: true,
  dts: true,
  sourcemap: false,
  minify: false,
  fixedExtension: false
});
