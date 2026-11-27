/**
 * Shared receipt check for the contract clients' write methods.
 */
import type { PublicClient } from 'viem';
import { ApiError } from './errors';
import { ErrorCode } from './types';

type Hex = `0x${string}`;

/**
 * Wait until a transaction is mined and fail unless it succeeded.
 *
 * viem resolves `waitForTransactionReceipt` for a mined transaction whether
 * or not it reverted. A transaction can pass gas estimation and still revert
 * when state changes before it is included (for example a second
 * `withdrawStake` in the same block), so the receipt status has to be read.
 *
 * @returns The transaction hash, once the receipt reports success
 * @throws {ApiError} With code `WZK-4002` (TransactionReverted) when the
 *   receipt does not report success; `details` holds the hash, block number
 *   and contract function.
 */
export async function waitForSuccess(
  publicClient: PublicClient,
  hash: Hex,
  functionName: string,
): Promise<Hex> {
  const receipt = await publicClient.waitForTransactionReceipt({ hash });
  if (receipt.status !== 'success') {
    throw new ApiError(
      ErrorCode.TransactionReverted,
      `${functionName} transaction ${hash} was mined in block ${receipt.blockNumber} but reverted`,
      0,
      { transactionHash: hash, blockNumber: receipt.blockNumber, functionName },
    );
  }
  return hash;
}
