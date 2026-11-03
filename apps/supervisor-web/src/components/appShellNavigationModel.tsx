import { translate, useI18n } from '@pockymoe/thread-ui/i18n';
import type {
  AgentBackendDto,
  AgentBackendIdDto,
  AgentBackendInstallationDto,
} from '../../../../packages/shared/src/index';
import {
  agentBackendIds,
  agentBackendMetadata,
  defaultAgentBackendId,
} from '../../../../packages/shared/src/index';
import type { AgentBackendId, ThemeMode } from './AppShellNavContext';

function menuItemClassName(active = false) {
  return `flex min-h-11 w-full items-center gap-3 rounded-md px-3 text-left text-sm font-medium transition ${
    active
      ? 'bg-[var(--theme-accent-soft)] text-[var(--theme-accent-strong)]'
      : 'text-[var(--theme-fg-soft)] hover:bg-[var(--theme-hover)] hover:text-[var(--theme-fg)]'
  }`;
}

const themeOptions: Array<{
  value: ThemeMode;
  label: string;
  description: string;
}> = [
  {
    value: 'light',
    get label() { return translate("files.light"); },
    get description() { return translate("files.alwaysUseTheBrightTheme"); },
  },
  {
    value: 'dark',
    get label() { return translate("files.dark"); },
    get description() { return translate("files.alwaysUseTheDarkTheme"); },
  },
  {
    value: 'system',
    get label() { return translate("files.system"); },
    get description() { return translate("files.followTheOperatingSystemAppearance"); },
  },
];

const emptyManagementSchema: AgentBackendDto['managementSchema'] = {
  hostConfigFiles: [],
  toolboxItems: [],
  hookCommandTemplates: [],
  providerConfigFormat: 'none',
  mcpConfigFormat: 'none',
  configArchives: false,
  buildRestart: false,
};

const backendInstallationFallbacks: Record<
  AgentBackendIdDto,
  Pick<
    AgentBackendInstallationDto,
    'packageName' | 'installCommand' | 'updateCommand'
  >
> = {
  codex: {
    packageName: '@openai/codex',
    installCommand: null,
    updateCommand: 'npm install -g @openai/codex@latest',
  },
  claude: {
    packageName: '@anthropic-ai/claude-agent-sdk',
    installCommand:
      'npm install -g @anthropic-ai/claude-code @anthropic-ai/claude-agent-sdk',
    updateCommand:
      'npm install -g @anthropic-ai/claude-code@latest @anthropic-ai/claude-agent-sdk@latest',
  },
  opencode: {
    packageName: 'opencode-ai',
    installCommand: 'npm install -g opencode-ai @opencode-ai/sdk',
    updateCommand: 'npm install -g opencode-ai@latest @opencode-ai/sdk@latest',
  },
  acp: {
    packageName: null,
    installCommand: null,
    updateCommand: null,
  },
};

function unavailableInstallation(
  provider: AgentBackendIdDto,
): AgentBackendInstallationDto {
  const fallback = backendInstallationFallbacks[provider];
  return {
    packageName: fallback.packageName,
    installed: provider === 'codex',
    installedVersion: null,
    latestVersion: null,
    installCommand: fallback.installCommand,
    updateCommand: fallback.updateCommand,
    busy: false,
    lastError: null,
  };
}

function unavailableBackend(
  provider: AgentBackendIdDto,
  displayName: string,
): AgentBackendDto {
  const descriptorUnavailable = provider === 'acp'
    ? translate("files.thisDeviceSupervisorDoesNotAdvertiseACP")
    : translate("files.backendDescriptorIsNotAvailable_f12b31");
  return {
    provider,
    displayName,
    description: translate("files.backendDescriptorIsNotAvailable", { value1: displayName }),
    enabled: false,
    isDefault: provider === defaultAgentBackendId,
    status: {
      state: 'stopped',
      transport: agentBackendMetadata[provider].defaultTransport,
      lastStartedAt: null,
      lastError: descriptorUnavailable,
      restartCount: 0,
    },
    capabilities: {
      sessions: {
        list: false,
        read: false,
        resume: false,
        importLocal: false,
      },
      turns: {
        start: false,
        streamInput: false,
        steer: false,
        interrupt: false,
        compact: false,
      },
      branching: {
        fork: false,
        hardRollback: false,
        resumeAt: false,
        rewindFiles: false,
      },
      controls: {
        planMode: false,
        permissionRequests: false,
        sandboxMode: false,
        performanceMode: false,
        goals: false,
      },
      management: {
        models: false,
        mcpStatus: false,
        skills: false,
        hooks: false,
        hookTrust: false,
        hostConfigFiles: false,
        providerSettings: false,
      },
      usage: {
        contextWindow: false,
        tokenUsage: false,
        costUsd: false,
      },
    },
    managementSchema: emptyManagementSchema,
    installation: unavailableInstallation(provider),
  };
}

function normalizeBackendDescriptor(backend: AgentBackendDto): AgentBackendDto {
  const installation =
    backend.installation ?? unavailableInstallation(backend.provider);
  return {
    ...backend,
    installation: {
      ...unavailableInstallation(backend.provider),
      ...installation,
    },
  };
}

const fallbackBackends: AgentBackendDto[] = [
  ...agentBackendIds.map((provider) =>
    unavailableBackend(provider, agentBackendMetadata[provider].displayName),
  ),
];

function fallbackManagementSchema(provider: AgentBackendId) {
  return (
    fallbackBackends.find((backend) => backend.provider === provider)
      ?.managementSchema ?? emptyManagementSchema
  );
}

export {
  emptyManagementSchema,
  fallbackBackends,
  fallbackManagementSchema,
  menuItemClassName,
  normalizeBackendDescriptor,
  themeOptions,
  unavailableBackend,
  unavailableInstallation,
};
