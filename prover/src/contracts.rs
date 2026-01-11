//! Smart contract interactions

use alloy::sol;

// Define the ExecutionEngine contract interface
sol! {
    #[sol(rpc)]
    interface IExecutionEngine {
        struct ExecutionRequest {
            uint256 id;
            address requester;
            bytes32 imageId;
            bytes32 inputDigest;
            string inputUrl;
            address callbackContract;
            uint256 tip;
            uint256 maxTip;
            uint256 createdAt;
            uint256 expiresAt;
            uint8 status;
            address claimedBy;
            uint256 claimedAt;
            uint256 claimDeadline;
        }

        function getRequest(uint256 requestId) external view returns (ExecutionRequest memory);
        function getPendingRequests(uint256 offset, uint256 limit) external view returns (uint256[] memory);
        function getCurrentTip(uint256 requestId) external view returns (uint256);
        function claimExecution(uint256 requestId) external;
        function submitProof(uint256 requestId, bytes calldata seal, bytes calldata journal) external;
    }

    #[sol(rpc)]
    interface IProgramRegistry {
        struct Program {
            bytes32 imageId;
            address owner;
            string name;
            string programUrl;
            bytes32 inputSchema;
            uint256 registeredAt;
            bool active;
        }

        function getProgram(bytes32 imageId) external view returns (Program memory);
        function isProgramActive(bytes32 imageId) external view returns (bool);
    }
}
