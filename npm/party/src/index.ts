/**
 * @tvpipe/party: party games on tvpipe. A server for the game's pages and
 * the phones' WebSockets, rooms with players, reconnects, actions and
 * state sync, the public link and its QR code, and the main screen on a
 * TV. See README.md and docs/party-design.md.
 *
 * @module
 */

export { createParty, socketsFor, toTv, consoleLogger, type CreatePartyOptions, type HostedParty, type PartyLinks, type TunnelChoice, type ToTvOptions, type ToTvWeb, type ToTvVideo, type TvHandle } from "./create.ts";
export {
  Party,
  PartyError,
  Player,
  PROTOCOL,
  BYE,
  type ActionHandler,
  type ByeReason,
  type CollectOptions,
  type JoinInfo,
  type Logger,
  type LogLevel,
  type PartyEvents,
  type PartyLimits,
  type PartyOptions,
  type PartyStats,
  type Role,
  type Screen,
  type Sender,
} from "./party.ts";
export { PartyServer, DEFAULT_CSP, type RouteHandler, type ServerOptions } from "./server.ts";
export { StateBox, diff, applyDiff, toJson, type Diff, type Json, type JsonObject } from "./sync.ts";
export { cleanText, graphemes, nameKey } from "./names.ts";
export { drawQr, qrToRgba } from "./qr.ts";
export type { QrRows, CanvasLike } from "./tvpipe.ts";
