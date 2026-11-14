import { connectPlugin, NATIVE_SERVICE_HOST_API_VERSION } from '@attson/plugin-sdk/browser';

import { createPluginRuntime } from './pluginRuntime';

const runtime = createPluginRuntime((appId) => connectPlugin(appId, {
  apiVersion: NATIVE_SERVICE_HOST_API_VERSION,
}));

export const adapter = runtime.adapter;
export const themeBridge = runtime.themeBridge;
