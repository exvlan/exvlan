import { resolve } from 'path'
import { defineConfig } from 'vite'
import vue from '@vitejs/plugin-vue'
import dts from 'unplugin-dts/vite';
import ViteYaml from '@modyfi/vite-plugin-yaml';
import tailwindcss from '@tailwindcss/vite'
// https://vite.dev/config/
export default defineConfig({
  // root: __dirname,
  resolve: {
    tsconfigPaths: true
  },
  plugins: [vue(), dts({
    processor: 'vue',
    outDirs: 'dist',
    staticImport: true,
    insertTypesEntry: true,
    copyDtsFiles: true,
    entryRoot: './src',
    beforeWriteFile: (filePath: string, content: string) => {
      return {
        filePath: filePath.replace('dist/src', 'dist'),
        content
      };
    },
    tsconfigPath: './tsconfig.app.json',
  }), ViteYaml(), tailwindcss()],
  build: {
    lib: {
      // Could also be a dictionary or array of multiple entry points
      entry: { index: resolve(__dirname, 'src/main.ts') },
      name: 'index',
      // the proper extensions will be added
      fileName: 'index',
      formats: ["es", "umd", "cjs"],
      cssFileName: 'style',
    },
    minify: true,
    rollupOptions: {
      input: {
        index: resolve(__dirname, "src/main.ts")
      },
      // make sure to externalize deps that shouldn't be bundled
      // into your library
      external: ['vue', 'primevue'],
      output: [{
        globals: {
          vue: 'Vue',
          primevue: 'primevue',
        },
        format: 'umd',
        name: 'Exvlan',
        entryFileNames: '[name].umd.js',
        exports: "named"
      }, {
        format: 'es',
        entryFileNames: '[name].js',
        exports: 'named'
      }, {
        format: 'cjs',
        entryFileNames: '[name].cjs',
        exports: 'named'
      }],
    },
  },
})
