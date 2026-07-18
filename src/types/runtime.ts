export interface RuntimeStatus {
  ready: boolean;
  phase: string;
  version: string;
  uptimeSeconds: number;
  capabilities: string[];
}
