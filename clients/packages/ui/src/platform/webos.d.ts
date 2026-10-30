/** Minimal ambient types for LG webOS globals (webOSTV.js / PalmSystem). All access must be guarded. */
interface WebOsDeviceInfo {
  modelName?: string;
  version?: string;
  versionMajor?: number;
  versionMinor?: number;
  sdkVersion?: string;
  screenWidth?: number;
  screenHeight?: number;
  uhd?: boolean;
  uhd8K?: boolean;
  hdr10?: boolean;
  dolbyVision?: boolean;
  dolbyAtmos?: boolean;
}
interface WebOsGlobal {
  platform?: { tv?: boolean; [k: string]: unknown };
  platformBack?: () => void;
  deviceInfo?: (cb: (info: WebOsDeviceInfo) => void) => void;
  fetchAppId?: () => string;
}
interface PalmSystemGlobal {
  platformBack?: () => void;
  deviceInfo?: string; // JSON string
  [k: string]: unknown;
}
declare var webOS: WebOsGlobal | undefined;
interface Window {
  PalmSystem?: PalmSystemGlobal;
  webOS?: WebOsGlobal;
}
