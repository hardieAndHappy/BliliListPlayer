import { describe, expect, it } from 'vitest';
import { getPageAccessErrorMessage } from './bilibiliPageState';

describe('getPageAccessErrorMessage', () => {
  it.each(['ready', 'guest'])('allows %s pages to continue', (state) => {
    expect(getPageAccessErrorMessage(state)).toBeNull();
  });

  it('asks for in-app verification only when Bilibili requires it', () => {
    const message = getPageAccessErrorMessage('verification-required');

    expect(message).toContain('应用内登录');
    expect(message).toContain('验证');
  });
});
