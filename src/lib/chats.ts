export const MAX_CHATS = 32;
export const MAX_DRAFT_LENGTH = 16384;
export const MAX_MESSAGE_BYTES = 16384;
export const MAX_MESSAGES = 128;
export const MAX_TRANSCRIPT_BYTES = 262144;
export const MAX_ASSISTANT_BYTES = 65536;
export function utf8Length(text: string): number { return new TextEncoder().encode(text).length; }
export function chatTitle(text: string): string { const first = text.trim().split('\n')[0]; return first.length > 22 ? `${first.slice(0, 22)}…` : first; }
