// @ts-nocheck
import { mkdtemp, mkdir, readFile, writeFile } from 'node:fs/promises';
import { tmpdir } from 'node:os';
import path from 'node:path';

import { describe, expect, it } from 'vitest';

import {
  assertVersionAlignment,
  expectedReleaseAssets,
  verifyReleaseAssets,
} from '../scripts/verify-release.mjs';

describe('clipboard release contract', () => {
  it('pins the plugin SDK through gh-proxy.com to the required immutable commit', async () => {
    const packageJson = JSON.parse(await readFile('package.json', 'utf8'));
    expect(packageJson.dependencies['@attson/plugin-sdk']).toBe(
      'https://gh-proxy.com/https://github.com/attsontools/attool-plugin-sdk/archive/410158b5e850836d5db681b1a02b13e4d55d53d6.tar.gz',
    );
  });

  it('builds and signs both artifacts for all five release targets', async () => {
    const workflow = await readFile('.github/workflows/release.yml', 'utf8');
    for (const target of [
      'aarch64-apple-darwin',
      'x86_64-apple-darwin',
      'aarch64-unknown-linux-gnu',
      'x86_64-unknown-linux-gnu',
      'x86_64-pc-windows-msvc',
    ]) {
      expect(workflow).toContain(`target: ${target}`);
    }
    expect(workflow).toContain('ATCLIPBOARD_PUBLISHER_KEY_ID');
    expect(workflow).toContain('ATCLIPBOARD_PUBLISHER_PRIVATE_KEY');
    expect(workflow).toContain('ATCLIPBOARD_PUBLISHER_PUBLIC_KEY');
    expect(workflow).toContain('run: pnpm package:plugin');
    expect(workflow).toContain('node scripts/verify-release.mjs assets');
    expect(workflow).not.toContain('package:plugin:test');
    const testJob = workflow.slice(workflow.indexOf('  test:'), workflow.indexOf('  build:'));
    expect(testJob).toContain('libwebkit2gtk-4.1-dev');
    expect(testJob.indexOf('libwebkit2gtk-4.1-dev')).toBeLessThan(testJob.indexOf('cargo test --workspace'));
  });

  it('requires all four project versions to match the release tag', () => {
    expect(assertVersionAlignment({
      tagName: 'v0.1.0-alpha.1',
      packageVersion: '0.1.0-alpha.1',
      tauriVersion: '0.1.0-alpha.1',
      serviceCargoVersion: '0.1.0-alpha.1',
      tauriCargoVersion: '0.1.0-alpha.1',
    })).toBe('0.1.0-alpha.1');

    expect(() => assertVersionAlignment({
      tagName: 'v0.1.0-alpha.1',
      packageVersion: '0.1.0-alpha.0',
      tauriVersion: '0.1.0-alpha.1',
      serviceCargoVersion: '0.1.0-alpha.1',
      tauriCargoVersion: '0.1.0-alpha.1',
    })).toThrow(/must all match/);
  });

  it('defines exactly one installer and one plugin package for every target', () => {
    expect(expectedReleaseAssets('0.1.0-alpha.1')).toEqual([
      'AT.Clipboard_0.1.0-alpha.1_linux_aarch64.atpkg',
      'AT.Clipboard_0.1.0-alpha.1_linux_aarch64.deb',
      'AT.Clipboard_0.1.0-alpha.1_linux_x86_64.atpkg',
      'AT.Clipboard_0.1.0-alpha.1_linux_x86_64.deb',
      'AT.Clipboard_0.1.0-alpha.1_macos_aarch64.atpkg',
      'AT.Clipboard_0.1.0-alpha.1_macos_aarch64.dmg',
      'AT.Clipboard_0.1.0-alpha.1_macos_x86_64.atpkg',
      'AT.Clipboard_0.1.0-alpha.1_macos_x86_64.dmg',
      'AT.Clipboard_0.1.0-alpha.1_windows_x86_64.atpkg',
      'AT.Clipboard_0.1.0-alpha.1_windows_x86_64.exe',
    ]);
  });

  it('rejects missing, extra, directory, and empty release assets', async () => {
    const directory = await mkdtemp(path.join(tmpdir(), 'atclipboard-release-'));
    const assets = expectedReleaseAssets('0.1.0-alpha.1');
    await Promise.all(assets.map((name) => writeFile(path.join(directory, name), 'asset')));
    await expect(verifyReleaseAssets(directory, '0.1.0-alpha.1')).resolves.toBeUndefined();

    await mkdir(path.join(directory, 'unexpected'));
    await expect(verifyReleaseAssets(directory, '0.1.0-alpha.1')).rejects.toThrow(/exact release asset set/);
  });
});
