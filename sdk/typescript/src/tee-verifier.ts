import {
  createPublicClient,
  createWalletClient,
  http,
  parseEther,
  type PublicClient,
  type WalletClient,
  type Transport,
  type Chain,
  defineChain,
} from 'viem';
import { privateKeyToAccount } from 'viem/accounts';
import { teeMLVerifierAbi } from './tee-verifier-abi';

type Hex = `0x${string}`;

export interface TEEVerifierConfig {
  rpcUrl: string;
  privateKey: Hex;
  contractAddress: Hex;
  chainId?: number;
}

export class TEEVerifier {
  private publicClient: PublicClient<Transport, Chain>;
  private walletClient: WalletClient<Transport, Chain>;
  private contractAddress: Hex;

  constructor(config: TEEVerifierConfig) {
    const chain = defineChain({
      id: config.chainId ?? 31337,
      name: 'Custom',
      nativeCurrency: { name: 'Ether', symbol: 'ETH', decimals: 18 },
      rpcUrls: { default: { http: [config.rpcUrl] } },
    });
    const transport = http(config.rpcUrl);
    const account = privateKeyToAccount(config.privateKey);

    this.publicClient = createPublicClient({ chain, transport });
    this.walletClient = createWalletClient({ chain, transport, account });
    this.contractAddress = config.contractAddress;
  }

  // -- Admin --

  async registerEnclave(enclaveKey: Hex, imageHash: Hex): Promise<Hex> {
    return this.send('registerEnclave', [enclaveKey, imageHash]);
  }

  async revokeEnclave(enclaveKey: Hex): Promise<Hex> {
    return this.send('revokeEnclave', [enclaveKey]);
  }

  // -- Submit --

  async submitResult(
    modelHash: Hex,
    inputHash: Hex,
    result: Hex,
    attestation: Hex,
    stakeEther = '0.1',
  ): Promise<Hex> {
    return this.send(
      'submitResult',
      [modelHash, inputHash, result, attestation],
      parseEther(stakeEther),
    );
  }

  // -- Challenge --

  async challenge(resultId: Hex, bondEther = '0.1'): Promise<Hex> {
    return this.send('challenge', [resultId], parseEther(bondEther));
  }

  // -- Finalize --

  async finalize(resultId: Hex): Promise<Hex> {
    return this.send('finalize', [resultId]);
  }

  // -- Dispute --

  async resolveDispute(
    resultId: Hex,
    proof: Hex,
    circuitHash: Hex,
    publicInputs: Hex,
    gensData: Hex,
  ): Promise<Hex> {
    return this.send('resolveDispute', [
      resultId,
      proof,
      circuitHash,
      publicInputs,
      gensData,
    ]);
  }

  async resolveDisputeByTimeout(resultId: Hex): Promise<Hex> {
    return this.send('resolveDisputeByTimeout', [resultId]);
  }

  // -- Query --

  async getResult(resultId: Hex) {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: teeMLVerifierAbi,
      functionName: 'getResult',
      args: [resultId],
    });
  }

  async isResultValid(resultId: Hex): Promise<boolean> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: teeMLVerifierAbi,
      functionName: 'isResultValid',
      args: [resultId],
    });
  }

  // -- Owner / Pausable (Ownable2Step) --

  async owner(): Promise<Hex> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: teeMLVerifierAbi,
      functionName: 'owner',
    });
  }

  async pendingOwner(): Promise<Hex> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: teeMLVerifierAbi,
      functionName: 'pendingOwner',
    });
  }

  async transferOwnership(newOwner: Hex): Promise<Hex> {
    return this.send('transferOwnership', [newOwner]);
  }

  async acceptOwnership(): Promise<Hex> {
    return this.send('acceptOwnership', []);
  }

  async pause(): Promise<Hex> {
    return this.send('pause', []);
  }

  async unpause(): Promise<Hex> {
    return this.send('unpause', []);
  }

  async paused(): Promise<boolean> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: teeMLVerifierAbi,
      functionName: 'paused',
    });
  }

  async remainderVerifier(): Promise<Hex> {
    return this.publicClient.readContract({
      address: this.contractAddress,
      abi: teeMLVerifierAbi,
      functionName: 'remainderVerifier',
    });
  }

  // -- Internal --

  // eslint-disable-next-line @typescript-eslint/no-explicit-any
  private async send(functionName: string, args: any[], value?: bigint): Promise<Hex> {
    const hash = await this.walletClient.writeContract({
      address: this.contractAddress,
      abi: teeMLVerifierAbi,
      functionName,
      args,
      ...(value !== undefined ? { value } : {}),
    } as any);
    await this.publicClient.waitForTransactionReceipt({ hash });
    return hash;
  }
}
