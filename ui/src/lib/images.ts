import type { UserImage } from '@/types/protocol';

export const MAX_IMAGES = 8;
export const MAX_IMAGE_BYTES = 5 * 1024 * 1024;
export const MAX_TOTAL_IMAGE_BYTES = 20 * 1024 * 1024;
export const IMAGE_ACCEPT = 'image/png,image/jpeg,image/gif,image/webp';
export interface DraftImage extends UserImage { id: string; name: string; size: number }

export function imageMediaType(bytes: Uint8Array): string | null {
  const at = (offset: number, values: number[]) => values.every((value, i) => bytes[offset + i] === value);
  if (at(0, [137, 80, 78, 71, 13, 10, 26, 10])) return 'image/png';
  if (at(0, [255, 216, 255])) return 'image/jpeg';
  if (at(0, [71, 73, 70, 56]) && (bytes[4] === 55 || bytes[4] === 57) && bytes[5] === 97) return 'image/gif';
  if (at(0, [82, 73, 70, 70]) && at(8, [87, 69, 66, 80])) return 'image/webp';
  return null;
}
export function validateImageBudget(sizes: number[]) {
  if (sizes.length > MAX_IMAGES) throw new Error('Maximum 8 images per message');
  if (sizes.some((size) => size <= 0 || size > MAX_IMAGE_BYTES)) throw new Error('Each image must be 1 byte–5 MiB');
  if (sizes.reduce((sum, size) => sum + size, 0) > MAX_TOTAL_IMAGE_BYTES) throw new Error('Images exceed 20 MiB total');
}
export async function importImage(file: File): Promise<DraftImage> {
  validateImageBudget([file.size]);
  if (!IMAGE_ACCEPT.split(',').includes(file.type)) throw new Error('Use PNG, JPEG, GIF or WebP images');
  const bytes = new Uint8Array(await file.arrayBuffer());
  const media_type = imageMediaType(bytes);
  if (!media_type || media_type !== file.type) throw new Error('Invalid image format');
  let binary = '';
  for (let i = 0; i < bytes.length; i += 32768) binary += String.fromCharCode(...bytes.subarray(i, i + 32768));
  const data = btoa(binary);
  // A signature alone can be forged; require the browser to decode the actual image.
  await new Promise<void>((resolve, reject) => {
    const image = new Image();
    const cleanup = () => { image.onload = null; image.onerror = null; image.src = ''; };
    image.onload = () => { cleanup(); resolve(); };
    image.onerror = () => { cleanup(); reject(new Error('Image could not be decoded')); };
    image.src = imageSource({ media_type, data });
  });
  return { id: crypto.randomUUID(), name: file.name, size: bytes.length, media_type, data };
}
export function imageSource(image: UserImage): string {
  return `data:${image.media_type};base64,${image.data}`;
}
export function wireImages(images: UserImage[]): UserImage[] {
  return images.map(({ media_type, data }) => ({ media_type, data }));
}
// Hide binary payloads wherever history/audit metadata is rendered as text.
export function metadataText(value: unknown): string {
  return JSON.stringify(value, (_key, item: unknown) => {
    if (item && typeof item === 'object' && 'media_type' in item && 'data' in item &&
      typeof item.media_type === 'string' && item.media_type.startsWith('image/')) {
      return { ...item, data: '[image data omitted]' };
    }
    return item;
  }, 2);
}
