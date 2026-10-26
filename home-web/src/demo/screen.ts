// A SYNTHETIC macOS-like desktop, drawn with the canvas API. It stands in for
// a real Mac's screen in demo mode: no screenshot of a real machine is used.

const W = 1280;
const H = 800;

function hue(seed: string): number {
  let h = 0;
  for (const ch of seed) h = (h * 31 + ch.charCodeAt(0)) % 360;
  return h;
}

function roundRect(ctx: CanvasRenderingContext2D, x: number, y: number, w: number, h: number, r: number) {
  ctx.beginPath();
  ctx.roundRect(x, y, w, h, r);
}

export function paintDesktop(ctx: CanvasRenderingContext2D, seed: string, name: string, marks: { x: number; y: number }[] = []) {
  const base = hue(seed);
  const sky = ctx.createLinearGradient(0, 0, W, H);
  sky.addColorStop(0, `hsl(${(base + 210) % 360} 55% 38%)`);
  sky.addColorStop(0.55, `hsl(${(base + 250) % 360} 50% 30%)`);
  sky.addColorStop(1, `hsl(${(base + 290) % 360} 45% 22%)`);
  ctx.fillStyle = sky;
  ctx.fillRect(0, 0, W, H);

  // Menu bar
  ctx.fillStyle = 'rgba(255,255,255,0.22)';
  ctx.fillRect(0, 0, W, 26);
  ctx.fillStyle = '#fff';
  ctx.font = '600 13px -apple-system, system-ui, sans-serif';
  ctx.fillText('Safari', 34, 18);
  ctx.font = '13px -apple-system, system-ui, sans-serif';
  ['File', 'Edit', 'View', 'History', 'Bookmarks', 'Window', 'Help'].forEach((item, i) => ctx.fillText(item, 92 + i * 62, 18));
  ctx.beginPath();
  ctx.arc(16, 13, 6, 0, Math.PI * 2);
  ctx.fill();
  const time = new Date().toLocaleTimeString('en', { hour: 'numeric', minute: '2-digit' });
  ctx.textAlign = 'right';
  ctx.fillText(time, W - 14, 18);
  ctx.textAlign = 'left';

  // A browser window
  ctx.save();
  ctx.shadowColor = 'rgba(0,0,0,0.35)';
  ctx.shadowBlur = 40;
  ctx.shadowOffsetY = 18;
  ctx.fillStyle = '#f6f6f7';
  roundRect(ctx, 150, 70, 900, 560, 12);
  ctx.fill();
  ctx.restore();
  ctx.fillStyle = '#e9e9eb';
  roundRect(ctx, 150, 70, 900, 52, 12);
  ctx.fill();
  ctx.fillRect(150, 100, 900, 22);
  [['#ff5f57', 0], ['#febc2e', 1], ['#28c840', 2]].forEach(([color, i]) => {
    ctx.fillStyle = color as string;
    ctx.beginPath();
    ctx.arc(174 + (i as number) * 20, 96, 6, 0, Math.PI * 2);
    ctx.fill();
  });
  ctx.fillStyle = '#fff';
  roundRect(ctx, 420, 82, 360, 28, 8);
  ctx.fill();
  ctx.fillStyle = '#6e6e73';
  ctx.font = '13px -apple-system, system-ui, sans-serif';
  ctx.fillText('example.test/notes', 540, 101);

  ctx.fillStyle = '#1d1d1f';
  ctx.font = '700 30px -apple-system, system-ui, sans-serif';
  ctx.fillText('Synthetic demo desktop', 200, 190);
  ctx.font = '16px -apple-system, system-ui, sans-serif';
  ctx.fillStyle = '#6e6e73';
  ctx.fillText(`${name} · not a real Mac`, 200, 220);
  for (let i = 0; i < 9; i += 1) {
    ctx.fillStyle = i % 4 === 0 ? '#d2d2d7' : '#e3e3e8';
    roundRect(ctx, 200, 260 + i * 36, 800 - ((i * 97) % 300), 14, 7);
    ctx.fill();
  }

  // Dock
  ctx.fillStyle = 'rgba(255,255,255,0.28)';
  roundRect(ctx, W / 2 - 290, H - 78, 580, 66, 18);
  ctx.fill();
  const tiles = ['#1e90ff', '#34c759', '#ff9500', '#ff3b30', '#af52de', '#5ac8fa', '#ffcc00', '#8e8e93'];
  tiles.forEach((color, i) => {
    ctx.fillStyle = color;
    roundRect(ctx, W / 2 - 270 + i * 68, H - 70, 50, 50, 12);
    ctx.fill();
  });

  // Where the person clicked while in control.
  for (const mark of marks) {
    ctx.fillStyle = 'rgba(10,132,255,0.85)';
    ctx.beginPath();
    ctx.arc(mark.x, mark.y, 7, 0, Math.PI * 2);
    ctx.fill();
  }
}

export function desktopCanvas(seed: string, name: string, marks: { x: number; y: number }[] = []): HTMLCanvasElement {
  const canvas = document.createElement('canvas');
  canvas.width = W;
  canvas.height = H;
  const ctx = canvas.getContext('2d');
  if (ctx) paintDesktop(ctx, seed, name, marks);
  return canvas;
}

const thumbs = new Map<string, { at: number; blob: Blob }>();
export async function thumbnail(seed: string, name: string): Promise<Blob> {
  const cached = thumbs.get(seed);
  if (cached && Date.now() - cached.at < 30_000) return cached.blob;
  const full = desktopCanvas(seed, name);
  const small = document.createElement('canvas');
  small.width = 640;
  small.height = 400;
  small.getContext('2d')?.drawImage(full, 0, 0, 640, 400);
  const blob = await new Promise<Blob>((resolve) => small.toBlob((b) => resolve(b ?? new Blob()), 'image/jpeg', 0.82));
  thumbs.set(seed, { at: Date.now(), blob });
  return blob;
}

/**
 * Just enough of an RFB 3.8 server for noVNC: no authentication, one raw
 * framebuffer update, and a dot wherever the viewer clicks in control mode.
 */
export class FakeScreenChannel {
  binaryType = 'arraybuffer';
  protocol = 'binary';
  readyState = 0;
  onopen: ((event: Event) => void) | null = null;
  onmessage: ((event: MessageEvent) => void) | null = null;
  onclose: ((event: CloseEvent) => void) | null = null;
  onerror: ((event: Event) => void) | null = null;
  private phase: 'version' | 'security' | 'init' | 'normal' = 'version';
  private buffer = new Uint8Array(0);
  private sentFull = false;
  private wantsUpdate = false;
  private marks: { x: number; y: number }[] = [];
  private seed: string;
  private name: string;

  constructor(seed: string, name: string) {
    this.seed = seed;
    this.name = name;
    setTimeout(() => {
      this.readyState = 1;
      this.onopen?.(new Event('open'));
      this.push(new TextEncoder().encode('RFB 003.008\n'));
    }, 120);
  }

  private push(bytes: Uint8Array) {
    if (this.readyState !== 1) return;
    const copy = bytes.slice().buffer;
    setTimeout(() => this.onmessage?.(new MessageEvent('message', { data: copy })), 0);
  }

  send(data: ArrayBuffer | Uint8Array) {
    const incoming = data instanceof Uint8Array ? data : new Uint8Array(data);
    const merged = new Uint8Array(this.buffer.length + incoming.length);
    merged.set(this.buffer);
    merged.set(incoming, this.buffer.length);
    this.buffer = merged;
    this.process();
  }

  close() {
    this.readyState = 3;
    this.onclose?.(new CloseEvent('close', { code: 1000, wasClean: true }));
  }

  private take(n: number): Uint8Array | null {
    if (this.buffer.length < n) return null;
    const out = this.buffer.slice(0, n);
    this.buffer = this.buffer.slice(n);
    return out;
  }

  private process() {
    for (;;) {
      if (this.phase === 'version') {
        if (!this.take(12)) return;
        this.phase = 'security';
        this.push(new Uint8Array([1, 1]));
        continue;
      }
      if (this.phase === 'security') {
        if (!this.take(1)) return;
        this.phase = 'init';
        this.push(new Uint8Array([0, 0, 0, 0]));
        continue;
      }
      if (this.phase === 'init') {
        if (!this.take(1)) return;
        this.phase = 'normal';
        const name = new TextEncoder().encode(this.name);
        const init = new Uint8Array(24 + name.length);
        const view = new DataView(init.buffer);
        view.setUint16(0, W);
        view.setUint16(2, H);
        init.set([32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 0, 8, 16, 0, 0, 0], 4);
        view.setUint32(20, name.length);
        init.set(name, 24);
        this.push(init);
        continue;
      }
      const type = this.buffer[0];
      if (type === undefined) return;
      const sizes: Record<number, number> = { 0: 20, 3: 10, 4: 8, 5: 6 };
      if (type === 2) {
        if (this.buffer.length < 4) return;
        const count = new DataView(this.buffer.buffer, this.buffer.byteOffset).getUint16(2);
        if (!this.take(4 + count * 4)) return;
      } else if (type === 6) {
        if (this.buffer.length < 8) return;
        const length = new DataView(this.buffer.buffer, this.buffer.byteOffset).getUint32(4);
        if (!this.take(8 + length)) return;
      } else if (sizes[type]) {
        const message = this.take(sizes[type]!);
        if (!message) return;
        if (type === 3) {
          this.wantsUpdate = true;
          if (!this.sentFull) this.sendFrame();
        }
        if (type === 5 && message[1]) {
          const view = new DataView(message.buffer);
          this.marks = [...this.marks.slice(-20), { x: view.getUint16(2), y: view.getUint16(4) }];
          if (this.wantsUpdate) this.sendFrame();
        }
      } else {
        // Unknown message: drop the buffer rather than misread it.
        this.buffer = new Uint8Array(0);
        return;
      }
    }
  }

  private sendFrame() {
    this.wantsUpdate = false;
    this.sentFull = true;
    const canvas = desktopCanvas(this.seed, this.name, this.marks);
    const pixels = canvas.getContext('2d')?.getImageData(0, 0, W, H).data;
    if (!pixels) return;
    const message = new Uint8Array(4 + 12 + pixels.length);
    const view = new DataView(message.buffer);
    message[0] = 0;
    view.setUint16(2, 1);
    view.setUint16(4, 0);
    view.setUint16(6, 0);
    view.setUint16(8, W);
    view.setUint16(10, H);
    view.setInt32(12, 0);
    message.set(pixels, 16);
    this.push(message);
  }
}
