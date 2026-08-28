export function getPageAccessErrorMessage(state: string): string | null {
  if (state !== 'verification-required') return null;

  return 'Bilibili 要求登录或验证，请点击「应用内登录」完成验证后再重试';
}
