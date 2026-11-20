/**
 * Coded JSON-RPC errors for the ACP surface.
 *
 * Repo discipline: production carries zero uncoded errors. A JSON-RPC error
 * object has a numeric `code` (the protocol's own vocabulary), so the harness
 * always adds a string `code` inside `data` — every failure a client sees
 * names itself ("session_not_found", "image_not_supported", …). Handlers wrap
 * their bodies with `normalizeError` so an unexpected throw still carries one.
 */

import { RequestError } from "@agentclientprotocol/sdk";

// The mapping helpers and tests name the SDK error type; this module is where
// the adapter's error vocabulary lives, so it re-exports the class.
export { RequestError };

/** The JSON-RPC numeric codes this adapter uses. */
export const JSONRPC_CODE = {
  parseError: -32700,
  invalidRequest: -32600,
  methodNotFound: -32601,
  invalidParams: -32602,
  internalError: -32603,
  requestCancelled: -32800,
  /** ACP's "resource not found"; `data.sessionId` names the session. */
  sessionNotFound: -32002,
} as const;

/** Builds a JSON-RPC error whose `data.code` carries the string code. */
export function codedError(
  numeric: number,
  stringCode: string,
  message: string,
  extra?: Record<string, unknown>,
): RequestError {
  return new RequestError(numeric, message, { code: stringCode, ...(extra ?? {}) });
}

/** The session-scoped error: `-32002` with `data.sessionId`, per the schema. */
export function sessionNotFoundError(sessionId: string): RequestError {
  return codedError(
    JSONRPC_CODE.sessionNotFound,
    "session_not_found",
    `session ${sessionId} is not active on this connection`,
    { sessionId },
  );
}

/**
 * Normalizes anything thrown into a `RequestError` that names a string code.
 * A `RequestError` without `data.code` gains the fallback; any other value
 * (pi throws plain `Error`s) becomes an internal error with the fallback.
 */
export function normalizeError(error: unknown, fallbackCode = "internal_error"): RequestError {
  if (error instanceof RequestError) {
    const data = isRecord(error.data) ? error.data : {};
    if (typeof data.code === "string" && data.code.length > 0) return error;
    return new RequestError(error.code, error.message, { code: fallbackCode, ...data });
  }
  const message = error instanceof Error ? error.message : String(error);
  return codedError(JSONRPC_CODE.internalError, fallbackCode, message.length > 0 ? message : "internal error");
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
