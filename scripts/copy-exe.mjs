// 编译产物拷贝：把 target/{profile}/biliListPlayer.exe 拷到根目录 output/。
// 用法：node scripts/copy-exe.mjs release | debug
// release → output/biliListPlayer.exe；debug → output/biliListPlayer-debug.exe。
// 这样 debug/release 不互相覆盖，且产物集中在 output 便于取用。
import { copyFile, mkdir } from 'node:fs/promises';
import { fileURLToPath } from 'node:url';
import { dirname, join } from 'node:path';

const root = join(dirname(fileURLToPath(import.meta.url)), '..');
const profile = process.argv[2];
if (profile !== 'release' && profile !== 'debug') {
  console.error('用法：node scripts/copy-exe.mjs release | debug');
  process.exit(1);
}
const src = join(root, 'src-tauri', 'target', profile, 'biliListPlayer.exe');
const outDir = join(root, 'output');
const dest = join(outDir, profile === 'release' ? 'biliListPlayer.exe' : 'biliListPlayer-debug.exe');

try {
  await mkdir(outDir, { recursive: true });
  await copyFile(src, dest);
  console.log(`✓ ${profile} 产物已拷贝到 ${dest.replace(root + '/', '')}`);
} catch (e) {
  console.error(`拷贝失败：${e.message}`);
  console.error(`（确认已先编译：src-tauri/target/${profile}/biliListPlayer.exe 需存在）`);
  process.exit(1);
}
