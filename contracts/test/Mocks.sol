// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;
import {Joah} from "src/Joah.sol";

/// Test-only deterministic oracle. Never deployed by production scripts.
contract MockDrand {
    mapping(uint64 => bytes32) private hashes;
    function set(uint64 round, bytes32 entropy) external { hashes[round] = entropy; }
    function verifyNormalized(uint64 round, bytes calldata signature)
        external view returns (bool, bytes32, bytes32)
    {
        bytes32 value = hashes[round];
        bool valid = value != bytes32(0) && (
            keccak256(signature) == keccak256(new bytes(48)) ||
            keccak256(signature) == keccak256(new bytes(96))
        );
        return (valid, value, value);
    }
}

contract JoahHarness is Joah {
    constructor(address treasury, uint256 supply, address oracle) Joah(treasury, supply, oracle) {}
    function ticket(bytes32 entropy) external pure returns (uint16) { return _uniformTicket(entropy); }
    function multiplier(uint16 value) external pure returns (uint8) { return _multiplier(value); }
    function distribution() external pure returns (uint16[10] memory counts) {
        for (uint16 value; value < 1023; ++value) ++counts[_multiplier(value)];
    }
}
