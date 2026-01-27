/**
 * World ZK Compute SDK - Error classes
 */

import { ApiErrorResponse, ErrorCode, RetryInfo } from './types';

const RETRYABLE_CODES = new Set([
  ErrorCode.RateLimited,
  ErrorCode.ServiceUnavailable,
  ErrorCode.ServiceOverloaded,
  ErrorCode.ProofTimeout,
  ErrorCode.BonsaiTimeout,
  ErrorCode.ChainUnavailable,
  ErrorCode.NetworkError,
  ErrorCode.ConnectionTimeout,
  ErrorCode.ConnectionRefused,
  ErrorCode.IpfsTimeout,
]);

const RETRY_DELAYS: Partial<Record<ErrorCode, number>> = {
  [ErrorCode.RateLimited]: 5000,
  [ErrorCode.ServiceOverloaded]: 10000,
  [ErrorCode.ServiceUnavailable]: 30000,
  [ErrorCode.ProofTimeout]: 60000,
  [ErrorCode.BonsaiTimeout]: 60000,
};

/**
 * Base error class for World ZK Compute SDK
 */
export class WorldZKError extends Error {
  constructor(message: string) {
    super(message);
    this.name = 'WorldZKError';
    Object.setPrototypeOf(this, WorldZKError.prototype);
  }
}

/**
 * API error with structured error code
 */
export class ApiError extends WorldZKError {
  readonly code: string;
  readonly httpStatus: number;
  readonly details?: Record<string, unknown>;
  readonly requestId?: string;
  readonly traceId?: string;
  readonly retry?: RetryInfo;

  constructor(
    code: string,
    message: string,
    httpStatus: number = 500,
    details?: Record<string, unknown>,
    requestId?: string,
    traceId?: string,
    retry?: RetryInfo
  ) {
    super(message);
    this.name = 'ApiError';
    this.code = code;
    this.httpStatus = httpStatus;
    this.details = details;
    this.requestId = requestId;
    this.traceId = traceId;
    this.retry = retry;
    Object.setPrototypeOf(this, ApiError.prototype);
  }

  /**
   * Create ApiError from API response
   */
  static fromResponse(data: ApiErrorResponse, httpStatus: number = 500): ApiError {
    const { code, message, details, requestId, traceId, retry } = data;

    // Determine the appropriate error class
    if (code.startsWith('WZK-103')) {
      return new AuthenticationError(code, message, httpStatus, details, requestId, traceId, retry);
    }
    if (code === 'WZK-1021' || code === 'WZK-3020') {
      return new NotFoundError(code, message, httpStatus, details, requestId, traceId, retry);
    }
    if (code === 'WZK-1040' || code === 'WZK-1041') {
      return new RateLimitError(code, message, httpStatus, details, requestId, traceId, retry);
    }
    if (code.startsWith('WZK-3')) {
      return new ProofError(code, message, httpStatus, details, requestId, traceId, retry);
    }
    if (code.startsWith('WZK-5')) {
      return new NetworkError(code, message, httpStatus, details, requestId, traceId, retry);
    }
    if (code.startsWith('WZK-1')) {
      return new ValidationError(code, message, httpStatus, details, requestId, traceId, retry);
    }

    return new ApiError(code, message, httpStatus, details, requestId, traceId, retry);
  }

  /**
   * Check if this error is retryable
   */
  get isRetryable(): boolean {
    return RETRYABLE_CODES.has(this.code as ErrorCode);
  }

  /**
   * Get suggested retry delay in milliseconds
   */
  get retryDelayMs(): number | undefined {
    if (this.retry?.retryAfterMs) {
      return this.retry.retryAfterMs;
    }
    return RETRY_DELAYS[this.code as ErrorCode] ?? (this.isRetryable ? 1000 : undefined);
  }

  toString(): string {
    let s = `[${this.code}] ${this.message}`;
    if (this.requestId) {
      s += ` (request_id: ${this.requestId})`;
    }
    return s;
  }
}

/**
 * Client-side validation error
 */
export class ValidationError extends ApiError {
  constructor(
    code: string,
    message: string,
    httpStatus: number = 400,
    details?: Record<string, unknown>,
    requestId?: string,
    traceId?: string,
    retry?: RetryInfo
  ) {
    super(code, message, httpStatus, details, requestId, traceId, retry);
    this.name = 'ValidationError';
    Object.setPrototypeOf(this, ValidationError.prototype);
  }
}

/**
 * Authentication or authorization error
 */
export class AuthenticationError extends ApiError {
  constructor(
    code: string,
    message: string,
    httpStatus: number = 401,
    details?: Record<string, unknown>,
    requestId?: string,
    traceId?: string,
    retry?: RetryInfo
  ) {
    super(code, message, httpStatus, details, requestId, traceId, retry);
    this.name = 'AuthenticationError';
    Object.setPrototypeOf(this, AuthenticationError.prototype);
  }
}

/**
 * Resource not found error
 */
export class NotFoundError extends ApiError {
  constructor(
    code: string,
    message: string,
    httpStatus: number = 404,
    details?: Record<string, unknown>,
    requestId?: string,
    traceId?: string,
    retry?: RetryInfo
  ) {
    super(code, message, httpStatus, details, requestId, traceId, retry);
    this.name = 'NotFoundError';
    Object.setPrototypeOf(this, NotFoundError.prototype);
  }
}

/**
 * Rate limit exceeded error
 */
export class RateLimitError extends ApiError {
  constructor(
    code: string,
    message: string,
    httpStatus: number = 429,
    details?: Record<string, unknown>,
    requestId?: string,
    traceId?: string,
    retry?: RetryInfo
  ) {
    super(code, message, httpStatus, details, requestId, traceId, retry);
    this.name = 'RateLimitError';
    Object.setPrototypeOf(this, RateLimitError.prototype);
  }

  /**
   * Get retry delay in seconds
   */
  get retryAfterSeconds(): number | undefined {
    const ms = this.retryDelayMs;
    return ms !== undefined ? ms / 1000 : undefined;
  }
}

/**
 * Proof generation or verification error
 */
export class ProofError extends ApiError {
  constructor(
    code: string,
    message: string,
    httpStatus: number = 500,
    details?: Record<string, unknown>,
    requestId?: string,
    traceId?: string,
    retry?: RetryInfo
  ) {
    super(code, message, httpStatus, details, requestId, traceId, retry);
    this.name = 'ProofError';
    Object.setPrototypeOf(this, ProofError.prototype);
  }
}

/**
 * Network or external service error
 */
export class NetworkError extends ApiError {
  constructor(
    code: string,
    message: string,
    httpStatus: number = 502,
    details?: Record<string, unknown>,
    requestId?: string,
    traceId?: string,
    retry?: RetryInfo
  ) {
    super(code, message, httpStatus, details, requestId, traceId, retry);
    this.name = 'NetworkError';
    Object.setPrototypeOf(this, NetworkError.prototype);
  }
}

/**
 * Operation timeout error
 */
export class TimeoutError extends WorldZKError {
  readonly timeoutMs: number;

  constructor(message: string = 'Operation timed out', timeoutMs: number = 0) {
    super(message);
    this.name = 'TimeoutError';
    this.timeoutMs = timeoutMs;
    Object.setPrototypeOf(this, TimeoutError.prototype);
  }
}
