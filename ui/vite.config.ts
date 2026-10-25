import fs from 'node:fs'
import path from 'node:path'
import tailwindcss from '@tailwindcss/vite'
import vue from '@vitejs/plugin-vue'
import { defineConfig, type Plugin } from 'vite'

// The node the dev server proxies to; override to point the UI at another
// instance, e.g. `CAPTURE_ROOM_NODE=localhost:7701 pnpm dev`.
const node = process.env.CAPTURE_ROOM_NODE ?? 'localhost:7700'

/**
 * Writes `third-party-licenses.txt` into the build: every npm package with code
 * in the bundle, with its license text. The app ships it with its other
 * notices (scripts/macos/package.sh).
 */
function thirdPartyLicenses(): Plugin {
  return {
    name: 'third-party-licenses',
    apply: 'build',
    generateBundle() {
      const packages = new Map<string, { name: string; version: string; license: string; text: string }>()
      for (const id of this.getModuleIds()) {
        const dir = packageDir(id)
        if (!dir || packages.has(dir)) continue
        const pkg = JSON.parse(fs.readFileSync(path.join(dir, 'package.json'), 'utf8'))
        const file = fs.readdirSync(dir).find((f) => /^(licen[cs]e|copying)/i.test(f))
        packages.set(dir, {
          name: pkg.name,
          version: pkg.version,
          license: typeof pkg.license === 'string' ? pkg.license : 'see below',
          text: file ? fs.readFileSync(path.join(dir, file), 'utf8').trim() : '(no license file in the package)',
        })
      }
      const rule = '='.repeat(80)
      const sections = [...packages.values()]
        .sort((a, b) => a.name.localeCompare(b.name))
        .map((p) => `${rule}\n${p.name} ${p.version} (${p.license})\n${rule}\n\n${p.text}\n`)
      this.emitFile({
        type: 'asset',
        fileName: 'third-party-licenses.txt',
        source: `npm packages built into the Capture Room UI, and their licenses.\n\n${sections.join('\n')}`,
      })
    },
  }
}

/** The root of the package a bundled module comes from, if it's from npm. */
function packageDir(id: string): string | null {
  const file = id.replace(/^\0/, '').split('?')[0]
  const at = file.lastIndexOf('/node_modules/')
  if (at < 0) return null
  const rest = file.slice(at + '/node_modules/'.length).split('/')
  const name = rest[0].startsWith('@') ? rest.slice(0, 2) : rest.slice(0, 1)
  const dir = file.slice(0, at) + '/node_modules/' + name.join('/')
  return fs.existsSync(path.join(dir, 'package.json')) ? dir : null
}

export default defineConfig({
  plugins: [vue(), tailwindcss(), thirdPartyLicenses()],
  resolve: {
    alias: {
      '@': path.resolve(__dirname, 'src'),
    },
  },
  server: {
    proxy: {
      '/api': `http://${node}`,
      '/ws': { target: `ws://${node}`, ws: true },
    },
  },
})
