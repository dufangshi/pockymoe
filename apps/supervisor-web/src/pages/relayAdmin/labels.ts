// Words for the codes the relay returns (events, scopes, visit kinds, admin
// actions, checks). Unknown codes show as they are rather than failing.
import { translate } from '@pockymoe/thread-ui/i18n';
import type { AdminRange, AdminShareScope } from './types';

export function eventLabel(event: string) {
  switch (event) {
    case 'login.mfa': return translate('admin.eventLoginMfa');
    case 'login.password_or_oauth': return translate('admin.eventLogin');
    case 'login.challenge': return translate('admin.eventChallenge');
    case 'login.failed': return translate('admin.eventLoginFailed');
    case 'security.reauthenticated': return translate('admin.eventReauth');
    case 'authenticator.enabled': return translate('admin.eventAuthenticatorEnabled');
    case 'authenticator.disabled': return translate('admin.eventAuthenticatorDisabled');
    case 'session.revoked': return translate('admin.eventSessionRevoked');
    case 'browser.revoked': return translate('admin.eventBrowserRevoked');
    case 'device.setup_copied': return translate('admin.eventSetupCopied');
    case 'device.token_rotated': return translate('admin.eventTokenRotated');
    case 'device.created': return translate('admin.eventDeviceCreated');
    case 'device.deleted': return translate('admin.eventDeviceDeleted');
    case 'passkey.added': return translate('admin.eventPasskeyAdded');
    case 'passkey.removed': return translate('admin.eventPasskeyRemoved');
    case 'share.created': return translate('admin.eventShareCreated');
    case 'share.revoked': return translate('admin.eventShareRevoked');
    case 'grant.created': return translate('admin.eventShareCreated');
    case 'grant.revoked': return translate('admin.eventShareRevoked');
    case 'registration.submitted': return translate('admin.eventRegistrationSubmitted');
    default: return event;
  }
}

export function eventCategoryLabel(category: string) {
  switch (category) {
    case 'login': return translate('admin.categoryLogin');
    case 'challenge': return translate('admin.categoryChallenge');
    case 'reauth': return translate('admin.categoryReauth');
    case 'failure': return translate('admin.categoryFailure');
    case 'activity': return translate('admin.categoryActivity');
    case 'credential': return translate('admin.categoryCredential');
    default: return category;
  }
}

export function scopeLabel(scope: AdminShareScope | string) {
  switch (scope) {
    case 'device': return translate('admin.scopeDevice');
    case 'workspace': return translate('admin.scopeWorkspace');
    case 'thread': return translate('admin.scopeThread');
    default: return scope;
  }
}

export function visitLabel(kind: string | null | undefined) {
  switch (kind) {
    case 'open_device': return translate('admin.visitOpenDevice');
    case 'open_thread': return translate('admin.visitOpenThread');
    case 'create_thread': return translate('admin.visitCreateThread');
    case 'send_prompt': return translate('admin.visitSendPrompt');
    case 'read_workspace_file': return translate('admin.visitReadFile');
    case 'write_workspace_file': return translate('admin.visitWriteFile');
    case null:
    case undefined: return '';
    default: return kind;
  }
}

export const VISIT_KINDS = ['open_thread', 'send_prompt', 'read_workspace_file', 'write_workspace_file', 'create_thread', 'open_device'] as const;

export function loginMethodLabel(method: string) {
  switch (method) {
    case 'password': return translate('admin.methodPassword');
    case 'google': return 'Google';
    case 'github': return 'GitHub';
    default: return method;
  }
}

export function actionLabel(action: string) {
  switch (action) {
    case 'registration.approved': return translate('admin.actionRegistrationApproved');
    case 'registration.rejected': return translate('admin.actionRegistrationRejected');
    case 'settings.registration': return translate('admin.actionRegistrationSettings');
    case 'settings.retention': return translate('admin.actionRetentionSettings');
    case 'user.enabled': return translate('admin.actionUserEnabled');
    case 'user.disabled': return translate('admin.actionUserDisabled');
    case 'user.deleted': return translate('admin.actionUserDeleted');
    case 'user.password_reset': return translate('admin.actionPasswordReset');
    case 'user.sessions_ended': return translate('admin.actionSessionsEnded');
    case 'session.ended': return translate('admin.actionSessionEnded');
    case 'device.disconnected': return translate('admin.actionDeviceDisconnected');
    case 'share.revoked': return translate('admin.actionShareRevoked');
    default: return action;
  }
}

export function checkLabel(key: string) {
  switch (key) {
    case 'integrity': return translate('admin.checkIntegrity');
    case 'sessionSecret': return translate('admin.checkSessionSecret');
    case 'publicBaseUrl': return translate('admin.checkPublicBaseUrl');
    case 'webOrigin': return translate('admin.checkWebOrigin');
    case 'github': return translate('admin.checkGithub');
    case 'google': return translate('admin.checkGoogle');
    case 'portPreviews': return translate('admin.checkPortPreviews');
    case 'apns': return translate('admin.checkApns');
    case 'legacyPublicLinks': return translate('admin.checkLegacyLinks');
    default: return key;
  }
}

export function accessLabel(access: 'read' | 'control') {
  return access === 'read' ? translate('admin.accessRead') : translate('admin.accessControl');
}

export function filesLabel(access: 'none' | 'read' | 'write') {
  return access === 'write' ? translate('admin.filesWrite') : access === 'read' ? translate('admin.filesRead') : translate('admin.filesNone');
}

export function protocolLabel(protocol: string) {
  switch (protocol) {
    case 'anthropic': return 'Anthropic';
    case 'openai_responses': return 'OpenAI Responses';
    case 'openai_chat': return 'OpenAI Chat';
    case 'gemini': return 'Gemini';
    default: return protocol;
  }
}

export function reasonLabel(reason: string | null) {
  switch (reason) {
    case 'closed': return translate('admin.reasonClosed');
    case 'timeout': return translate('admin.reasonTimeout');
    case 'replaced': return translate('admin.reasonReplaced');
    case 'queue_full': return translate('admin.reasonQueueFull');
    case 'deleted': return translate('admin.reasonDeleted');
    case 'token_rotated': return translate('admin.reasonTokenRotated');
    case 'admin': return translate('admin.reasonAdmin');
    case 'relay_stopped': return translate('admin.reasonRelayStopped');
    case null: return '';
    default: return reason;
  }
}

export function rangeLabel(range: AdminRange) {
  switch (range) {
    case '24h': return translate('admin.range24h');
    case '7d': return translate('admin.range7d');
    case '30d': return translate('admin.range30d');
    case '90d': return translate('admin.range90d');
  }
}
