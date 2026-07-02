import { fileToDataUrl } from './image';

describe('image helpers', () => {
  it('converts a browser File to a data URL', async () => {
    const file = new File([new Uint8Array([104, 105])], 'pic.png', { type: 'image/png' });

    await expect(fileToDataUrl(file)).resolves.toBe('data:image/png;base64,aGk=');
  });
});
