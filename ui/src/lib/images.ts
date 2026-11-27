import { randomId } from '@/lib/id';
import type { UserImage } from '@/types/protocol';

export const MAX_IMAGES = 8;
export const MAX_IMAGE_DIMENSION = 1280;
export const IMAGE_QUALITY = 0.8;
export const IMAGE_TARGET_BYTES = 307200;
export const IMAGE_ACCEPT = 'image/png,image/jpeg,image/gif,image/webp';
export interface DraftImage extends UserImage {
  id: string;
  name: string;
  size: number;
}

export function imageMediaType(bytes: Uint8Array): string | null {
  const at = (offset: number, values: number[]) =>
    values.every((value, index) => bytes[offset + index] === value);
  if (at(0, [137, 80, 78, 71, 13, 10, 26, 10])) return 'image/png';
  if (at(0, [255, 216, 255])) return 'image/jpeg';
  if (at(0, [71, 73, 70, 56]) && (bytes[4] === 55 || bytes[4] === 57) && bytes[5] === 97)
    return 'image/gif';
  if (at(0, [82, 73, 70, 70]) && at(8, [87, 69, 66, 80])) return 'image/webp';
  return null;
}

export function validateImageBudget(sizes: number[]) {
  if (sizes.length > MAX_IMAGES) throw new Error('Maximum 8 images per message');
}

interface DecodedImage {
  source: CanvasImageSource;
  width: number;
  height: number;
  close: () => void;
}

async function decodeImage(file: File): Promise<DecodedImage> {
  if (typeof createImageBitmap === 'function') {
    try {
      const bitmap = await createImageBitmap(file);
      return {
        source: bitmap,
        width: bitmap.width,
        height: bitmap.height,
        close: () => bitmap.close(),
      };
    } catch {
      // Some browsers reject large/oddly encoded files in createImageBitmap
      // while the ordinary <img> decoder can still decode them.
    }
  }
  const url = URL.createObjectURL(file);
  const image = new Image();
  try {
    await new Promise<void>((resolve, reject) => {
      image.onload = () => resolve();
      image.onerror = () => reject(new Error('Image could not be decoded'));
      image.src = url;
    });
    return {
      source: image,
      width: image.naturalWidth,
      height: image.naturalHeight,
      close: () => URL.revokeObjectURL(url),
    };
  } catch (error) {
    URL.revokeObjectURL(url);
    throw error;
  }
}

function scaledSize(width: number, height: number, maxDimension: number) {
  const scale = Math.min(1, maxDimension / width, maxDimension / height);
  return {
    width: Math.max(1, Math.round(width * scale)),
    height: Math.max(1, Math.round(height * scale)),
  };
}

function draw(source: CanvasImageSource, width: number, height: number): HTMLCanvasElement {
  const canvas = document.createElement('canvas');
  canvas.width = width;
  canvas.height = height;
  const context = canvas.getContext('2d', { alpha: true });
  if (!context) throw new Error('Image canvas is unavailable');
  context.drawImage(source, 0, 0, width, height);
  return canvas;
}

function hasTransparency(canvas: HTMLCanvasElement): boolean {
  const context = canvas.getContext('2d', { alpha: true });
  if (!context) return false;
  const data = context.getImageData(0, 0, canvas.width, canvas.height).data;
  for (let index = 3; index < data.length; index += 4) {
    if (data[index]! < 255) return true;
  }
  return false;
}

function encode(canvas: HTMLCanvasElement, type: string, quality: number): Promise<Blob> {
  return new Promise((resolve, reject) => {
    canvas.toBlob(
      (blob) => (blob ? resolve(blob) : reject(new Error('Image encoding failed'))),
      type,
      quality,
    );
  });
}

async function imageData(blob: Blob): Promise<string> {
  const bytes = new Uint8Array(await blob.arrayBuffer());
  let binary = '';
  for (let index = 0; index < bytes.length; index += 32768)
    binary += String.fromCharCode(...bytes.subarray(index, index + 32768));
  return btoa(binary);
}

/**
 * Mirrors the read_image preview policy: decode the actual source, never reject
 * solely for its original byte size, cap the long edge, preserve transparency
 * when it exists, and shrink until the preview fits the target byte budget.
 */
export async function importImage(file: File): Promise<DraftImage> {
  const head = new Uint8Array(await file.slice(0, 16).arrayBuffer());
  const sourceType = imageMediaType(head);
  if (!sourceType) throw new Error('Use PNG, JPEG, GIF or WebP images');
  const decoded = await decodeImage(file);
  try {
    const initial = scaledSize(decoded.width, decoded.height, MAX_IMAGE_DIMENSION);
    let width = initial.width;
    let height = initial.height;
    for (let attempt = 0; attempt < 8; attempt += 1) {
      const canvas = draw(decoded.source, width, height);
      const transparent = hasTransparency(canvas);
      const type = transparent ? 'image/webp' : 'image/jpeg';
      const qualities = transparent ? [0.9, 0.8, 0.7] : [IMAGE_QUALITY, 0.7, 0.6];
      let smallest: Blob | null = null;
      for (const quality of qualities) {
        const blob = await encode(canvas, type, quality);
        if (!smallest || blob.size < smallest.size) smallest = blob;
        if (blob.size <= IMAGE_TARGET_BYTES) {
          // A browser may fall back to another encoder when the requested
          // WebP/JPEG type is unavailable. Trust the returned MIME type.
          const mediaType = blob.type || type;
          if (!IMAGE_ACCEPT.split(',').includes(mediaType))
            throw new Error('Image encoding produced an unsupported format');
          return {
            id: randomId(),
            name: file.name,
            size: blob.size,
            media_type: mediaType,
            data: await imageData(blob),
          };
        }
      }
      if (width === 1 && height === 1) throw new Error('Image cannot fit the preview size limit');
      const ratio = Math.max(
        0.5,
        Math.min(
          0.9,
          Math.sqrt(IMAGE_TARGET_BYTES / (smallest?.size ?? IMAGE_TARGET_BYTES)) * 0.95,
        ),
      );
      width = Math.max(1, Math.floor(width * ratio));
      height = Math.max(1, Math.floor(height * ratio));
    }
    throw new Error('Image cannot fit the preview size limit');
  } finally {
    decoded.close();
  }
}

export function imageSource(image: UserImage): string {
  return `data:${image.media_type};base64,${image.data}`;
}
export function wireImages(images: UserImage[]): UserImage[] {
  return images.map(({ media_type, data }) => ({ media_type, data }));
}
// Hide binary payloads wherever history/audit metadata is rendered as text.
export function metadataText(value: unknown): string {
  return JSON.stringify(
    value,
    (_key, item: unknown) => {
      if (
        item &&
        typeof item === 'object' &&
        'media_type' in item &&
        'data' in item &&
        typeof item.media_type === 'string' &&
        item.media_type.startsWith('image/')
      ) {
        return { ...item, data: '[image data omitted]' };
      }
      return item;
    },
    2,
  );
}
