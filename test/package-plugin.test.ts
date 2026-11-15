// @ts-nocheck
import { generateKeyPairSync } from 'node:crypto';

import { describe, expect, it } from 'vitest';

import {
  createPluginManifest,
  loadPublisherSigningConfig,
  requiredPayloadPaths,
  resolveReleaseTarget,
} from '../scripts/package-plugin.mjs';

describe('clipboard plugin package contract', () => {
  it('declares the signed auxiliary windows and background clipboard service', () => {
    const target = resolveReleaseTarget('x86_64-unknown-linux-gnu');
    const files = {
      LICENSE: 'a'.repeat(64),
      'ui/index.html': 'b'.repeat(64),
      'ui/history.html': 'c'.repeat(64),
      'ui/preview.html': 'd'.repeat(64),
      'native/atclipboard-service': 'e'.repeat(64),
    };

    const manifest = createPluginManifest({
      version: '0.1.0-alpha.0',
      target,
      publisherKeyId: 'attool-p0-test',
      files,
    });

    expect(manifest.appId).toBe('com.attson.clipboard');
    expect(manifest.hostApi).toEqual({ min: '2.0.0', max: '2.x' });
    expect(manifest.permissions).toEqual([
      'host.theme.read',
      'host.ui.auxiliary',
      'host.clipboard.history',
    ]);
    expect(manifest.ui).toEqual({
      entry: 'ui/index.html',
      windows: {
        history: { entry: 'ui/history.html', presentation: 'bottomPanel' },
        preview: { entry: 'ui/preview.html', presentation: 'centeredDialog' },
      },
    });
    expect(manifest.nativeService).toEqual({
      entry: 'native/atclipboard-service',
      version: '0.1.0-alpha.0',
      sha256: 'e'.repeat(64),
      protocolVersion: 1,
      lifecycle: 'background',
    });
  });

  it('requires all three UI entries and the target service executable', () => {
    expect(requiredPayloadPaths(resolveReleaseTarget('x86_64-unknown-linux-gnu'))).toEqual([
      'LICENSE',
      'native/atclipboard-service',
      'ui/history.html',
      'ui/index.html',
      'ui/preview.html',
    ]);
    expect(requiredPayloadPaths(resolveReleaseTarget('x86_64-pc-windows-msvc')))
      .toContain('native/atclipboard-service.exe');
  });

  it('maps all five release targets and rejects unknown triples', () => {
    const targets = [
      ['aarch64-apple-darwin', 'macos', 'aarch64', 'atclipboard-service'],
      ['x86_64-apple-darwin', 'macos', 'x86_64', 'atclipboard-service'],
      ['aarch64-unknown-linux-gnu', 'linux', 'aarch64', 'atclipboard-service'],
      ['x86_64-unknown-linux-gnu', 'linux', 'x86_64', 'atclipboard-service'],
      ['x86_64-pc-windows-msvc', 'windows', 'x86_64', 'atclipboard-service.exe'],
    ];
    for (const [triple, targetOs, targetArch, serviceFileName] of targets) {
      expect(resolveReleaseTarget(triple)).toMatchObject({
        targetOs,
        targetArch,
        serviceFileName,
      });
    }
    expect(() => resolveReleaseTarget('wasm32-unknown-unknown')).toThrow(/Unsupported/);
  });

  it('accepts only a matching Ed25519 production Publisher key pair', () => {
    const keys = generateKeyPairSync('ed25519');
    const publicJwk = keys.publicKey.export({ format: 'jwk' });
    const publicKey = Buffer.from(publicJwk.x, 'base64url').toString('base64');
    const privateKey = keys.privateKey.export({ format: 'pem', type: 'pkcs8' }).toString();

    const signing = loadPublisherSigningConfig({
      ATCLIPBOARD_PUBLISHER_KEY_ID: 'attson-clipboard-2026-01',
      ATCLIPBOARD_PUBLISHER_PRIVATE_KEY: privateKey,
      ATCLIPBOARD_PUBLISHER_PUBLIC_KEY: publicKey,
    });

    expect(signing.keyId).toBe('attson-clipboard-2026-01');
    expect(signing.privateKey.asymmetricKeyType).toBe('ed25519');
    expect(signing.publicKey.asymmetricKeyType).toBe('ed25519');
  });

  it('rejects missing, malformed, and mismatched production Publisher keys', () => {
    const first = generateKeyPairSync('ed25519');
    const second = generateKeyPairSync('ed25519');
    const privateKey = first.privateKey.export({ format: 'pem', type: 'pkcs8' }).toString();
    const secondJwk = second.publicKey.export({ format: 'jwk' });
    const wrongPublicKey = Buffer.from(secondJwk.x, 'base64url').toString('base64');

    expect(() => loadPublisherSigningConfig({})).toThrow(/KEY_ID is required/);
    expect(() => loadPublisherSigningConfig({
      ATCLIPBOARD_PUBLISHER_KEY_ID: 'attson-clipboard-2026-01',
      ATCLIPBOARD_PUBLISHER_PRIVATE_KEY: 'not-a-key',
      ATCLIPBOARD_PUBLISHER_PUBLIC_KEY: wrongPublicKey,
    })).toThrow(/must be an Ed25519 private key/);
    expect(() => loadPublisherSigningConfig({
      ATCLIPBOARD_PUBLISHER_KEY_ID: 'attson-clipboard-2026-01',
      ATCLIPBOARD_PUBLISHER_PRIVATE_KEY: privateKey,
      ATCLIPBOARD_PUBLISHER_PUBLIC_KEY: wrongPublicKey,
    })).toThrow(/does not match configured public key/);
  });
});
