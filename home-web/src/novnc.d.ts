// The parts of noVNC's RFB class the portal uses (noVNC ships no types).
declare module '@novnc/novnc' {
  export type RFBOptions = { shared?: boolean; credentials?: { password?: string }; wsProtocols?: string[] };
  export default class RFB extends EventTarget {
    constructor(target: HTMLElement, urlOrChannel: string | object, options?: RFBOptions);
    viewOnly: boolean;
    scaleViewport: boolean;
    /** Clip to the container instead of scrolling it. */
    clipViewport: boolean;
    /** Drags pan a clipped viewport instead of reaching the remote mouse. */
    dragViewport: boolean;
    readonly clippingViewport: boolean;
    resizeSession: boolean;
    focusOnClick: boolean;
    showDotCursor: boolean;
    background: string;
    /** 0 (smallest JPEG) to 9 (best). */
    qualityLevel: number;
    /** 0 (least CPU) to 9 (least bandwidth). */
    compressionLevel: number;
    disconnect(): void;
    sendKey(keysym: number, code: string | null, down?: boolean): void;
    sendCtrlAltDel(): void;
    focus(options?: FocusOptions): void;
    blur(): void;
    clipboardPasteFrom(text: string): void;
    /** The whole framebuffer, at its own resolution. */
    toBlob(callback: (blob: Blob | null) => void, type?: string, quality?: number): void;
  }
}
