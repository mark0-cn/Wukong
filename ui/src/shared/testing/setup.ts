import { afterEach } from 'vitest';
import { cleanup } from '@testing-library/react';
Object.defineProperty(navigator, 'language', { value: 'zh-CN', configurable: true });
afterEach(cleanup);
