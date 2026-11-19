import { apiRequest } from '../client';
import type { Config, ConfigPatch, Health, Stats } from '../types/config';
import type { CallOptions } from './common';

export function getHealth(options: CallOptions = {}): Promise<Health> {
  return apiRequest<Health>('/api/health', options);
}

export function getConfig(options: CallOptions = {}): Promise<Config> {
  return apiRequest<Config>('/api/config', options);
}

export function patchConfig(patch: ConfigPatch, options: CallOptions = {}): Promise<Config> {
  return apiRequest<Config>('/api/config', { ...options, method: 'PATCH', body: patch });
}

export function getStats(options: CallOptions = {}): Promise<Stats> {
  return apiRequest<Stats>('/api/stats', options);
}
