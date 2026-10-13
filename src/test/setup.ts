import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/react';
afterEach(cleanup);
Object.defineProperty(HTMLElement.prototype, 'scrollTo', { value() {}, writable: true });
