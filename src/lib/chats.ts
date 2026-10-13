export const MAX_CHATS = 30;
export const MAX_DRAFT_LENGTH = 16000;
export const MAX_MESSAGES = 40;
export type LocalMessage = { id: string; text: string; createdAt: number };
export type LocalChat = { id: string; title: string; draft: string; messages: LocalMessage[] };
export function createLocalChat(): LocalChat { return { id: crypto.randomUUID(), title: '新任务', draft: '', messages: [] }; }
export function chatTitle(text: string): string { const first = text.trim().split('\n')[0]; return first.length > 22 ? `${first.slice(0,22)}…` : first; }
