// SPDX-License-Identifier: MIT
pragma solidity 0.8.30;

import {ERC20} from "@openzeppelin/contracts/token/ERC20/ERC20.sol";

interface IDrandQuicknet {
    function verifyNormalized(uint64 round, bytes calldata signature)
        external view returns (bool ok, bytes32 normalizedRoundHash, bytes32 chainScopedHash);
}

/// @title 좋아 (✨), ClipClub's MegaETH currency and verifiable rolls.
/// @notice Stakes remain escrowed until the committed drand quicknet round is
/// verified. Anyone may settle. There is no privileged mint, refund, or reroll.
contract Joah is ERC20 {
    uint256 public constant UNIT = 1_000_000;
    uint256 public constant QUICKNET_GENESIS = 1_692_803_367;
    uint256 public constant QUICKNET_PERIOD = 3;
    uint256 public constant MAX_WAGER = type(uint256).max / 9;
    uint256 public constant MAX_RATINGS_PER_SESSION = 100;
    address public constant MAINNET_VERIFIER = 0x7a53a6eFA81c426838fcf4824E6e207923969b36;
    address public constant TESTNET_VERIFIER = 0x4e1673dcAA38136b5032F27ef93423162aF977Cc;

    IDrandQuicknet public immutable verifier;
    uint256 public immutable genesisSupply;
    uint256 public nextRollId = 1;
    uint256 public totalBurned;
    uint256 public totalWinMinted;
    /// @dev Reserve headroom for every pending 9x payout, preventing overflow
    /// from making a previously accepted roll impossible to settle.
    uint256 public reservedWinProfit;

    struct Roll {
        address player;
        uint64 round;
        bool settled;
        uint8 multiplier;
        uint256 wager;
    }
    mapping(uint256 => Roll) private _rolls;
    mapping(address => uint256) public pendingRoll;
    mapping(bytes32 => address) public clipBeneficiary;
    mapping(bytes32 => mapping(address => bool)) public hasRated;
    mapping(address => mapping(bytes32 => bool)) public settledSession;

    event RollRequested(uint256 indexed id, address indexed player, uint256 wager, uint64 round);
    event RollSettled(uint256 indexed id, address indexed player, uint256 wager, uint256 payout, uint8 multiplier, uint64 round);
    event ClipRegistered(bytes32 indexed clipId, address indexed beneficiary);
    event ClipRated(bytes32 indexed clipId, address indexed rater, uint8 score, uint256 amount, bytes32 peerCommitment);
    event RatingsSettled(bytes32 indexed sessionId, address indexed rater, uint256 count, uint256 amount, bytes32 peerCommitment);

    error UnsupportedNetwork();
    error InvalidVerifier();
    error InvalidTreasury();
    error InvalidWager();
    error SupplyCapacityExceeded();
    error UnknownRoll();
    error AlreadySettled();
    error PendingRollExists();
    error RoundUnavailable();
    error InvalidProof();
    error InvalidClip();
    error ClipAlreadyRegistered();
    error InvalidRating();
    error AlreadyRated();
    error InvalidRatingSession();
    error RatingSessionAlreadySettled();

    /// @param initialSupply Supply in 6-decimal base units, sent once to treasury.
    /// Only chain 31337 permits a custom verifier for isolated local tests.
    constructor(address treasury, uint256 initialSupply, address verifierAddress)
        ERC20(unicode"좋아", unicode"✨")
    {
        if (treasury == address(0) || treasury == address(this)) revert InvalidTreasury();
        if (block.chainid == 4326) {
            if (verifierAddress != MAINNET_VERIFIER) revert InvalidVerifier();
        } else if (block.chainid == 6343) {
            if (verifierAddress != TESTNET_VERIFIER) revert InvalidVerifier();
        } else if (block.chainid != 31337) {
            revert UnsupportedNetwork();
        }
        if (verifierAddress.code.length == 0) revert InvalidVerifier();
        verifier = IDrandQuicknet(verifierAddress);
        genesisSupply = initialSupply;
        _mint(treasury, initialSupply);
    }

    function decimals() public pure override returns (uint8) { return 6; }

    /// @notice A fixed output order for RPC clients; storage remains packed.
    function rolls(uint256 id) external view
        returns (address player, uint256 wager, uint64 round, bool settled, uint8 multiplier)
    {
        Roll storage r = _rolls[id];
        return (r.player, r.wager, r.round, r.settled, r.multiplier);
    }

    function requestRoll(uint256 wager) external returns (uint256 id) {
        if (pendingRoll[msg.sender] != 0) revert PendingRollExists();
        if (wager == 0 || wager > MAX_WAGER) revert InvalidWager();
        uint256 maxProfit = wager * 8;
        uint256 tracked = totalSupply();
        if (totalWinMinted > tracked) tracked = totalWinMinted;
        if (totalBurned > tracked) tracked = totalBurned;
        if (maxProfit > type(uint256).max - tracked - reservedWinProfit) {
            revert SupplyCapacityExceeded();
        }
        // No allowance or external token callback: escrow this ERC20 directly.
        _transfer(msg.sender, address(this), wager);
        reservedWinProfit += maxProfit;
        id = nextRollId++;
        pendingRoll[msg.sender] = id;
        // Read volatile block time once, after ordinary bookkeeping (MegaEVM).
        uint256 timestamp = block.timestamp;
        if (timestamp < QUICKNET_GENESIS) revert RoundUnavailable();
        uint256 round = (timestamp - QUICKNET_GENESIS) / QUICKNET_PERIOD + 3;
        if (round > type(uint64).max || QUICKNET_GENESIS + (round - 1) * QUICKNET_PERIOD <= timestamp) {
            revert RoundUnavailable();
        }
        _rolls[id] = Roll(msg.sender, uint64(round), false, 0, wager);
        emit RollRequested(id, msg.sender, wager, uint64(round));
    }

    function settleRoll(uint256 id, bytes calldata signature) external {
        Roll storage r = _rolls[id];
        if (r.player == address(0)) revert UnknownRoll();
        if (r.settled) revert AlreadySettled();
        if (signature.length != 48 && signature.length != 96) revert InvalidProof();
        if (block.timestamp < QUICKNET_GENESIS + (uint256(r.round) - 1) * QUICKNET_PERIOD) {
            revert RoundUnavailable();
        }
        (bool ok,, bytes32 chainScopedHash) = verifier.verifyNormalized(r.round, signature);
        if (!ok) revert InvalidProof();
        bytes32 entropy = keccak256(abi.encode(
            chainScopedHash, address(this), block.chainid, id, r.player, r.wager, r.round
        ));
        uint8 multiplier = _multiplier(_uniformTicket(entropy));
        r.settled = true;
        r.multiplier = multiplier;
        delete pendingRoll[r.player];
        reservedWinProfit -= r.wager * 8;
        uint256 payout = r.wager * multiplier;
        if (multiplier == 0) {
            totalBurned += r.wager;
            _burn(address(this), r.wager);
        } else {
            _transfer(address(this), r.player, r.wager);
            if (multiplier > 1) {
                uint256 profit = payout - r.wager;
                totalWinMinted += profit;
                _mint(r.player, profit);
            }
        }
        emit RollSettled(id, r.player, r.wager, payout, multiplier, r.round);
    }

    /// @notice First on-chain registration fixes the recipient for a content ID.
    /// This registers a payment recipient, not a claim of copyright/ownership.
    function registerClip(bytes32 clipId) external {
        if (clipId == bytes32(0)) revert InvalidClip();
        if (clipBeneficiary[clipId] != address(0)) revert ClipAlreadyRegistered();
        clipBeneficiary[clipId] = msg.sender;
        emit ClipRegistered(clipId, msg.sender);
    }

    /// @notice Ratings transfer existing tokens; they never manufacture rewards
    /// from unverifiable peer counts. A free skip is handled locally by the app.
    /// @param peerCommitment SHA-256 of the rater's ClipClub peer ID. Binding it
    /// on-chain prevents another peer from claiming a public payment receipt.
    function rateClip(bytes32 clipId, uint8 score, bytes32 peerCommitment) external {
        if (peerCommitment == bytes32(0)) revert InvalidRating();
        address beneficiary = _ratingBeneficiary(clipId, score);
        hasRated[clipId][msg.sender] = true;
        _payRating(clipId, score, beneficiary, peerCommitment);
    }

    /// @notice Settle a locally reviewed rating session in one wallet transaction.
    /// Every item must succeed; any failure reverts transfers, flags, and logs.
    /// The session ID is a fixed off-chain random identifier, scoped by wallet.
    function rateClips(
        bytes32 sessionId,
        bytes32[] calldata clipIds,
        uint8[] calldata scores,
        bytes32 peerCommitment
    ) external {
        uint256 count = clipIds.length;
        if (sessionId == bytes32(0) || peerCommitment == bytes32(0)
            || count == 0 || count > MAX_RATINGS_PER_SESSION || count != scores.length) {
            revert InvalidRatingSession();
        }
        if (settledSession[msg.sender][sessionId]) revert RatingSessionAlreadySettled();
        address[] memory beneficiaries = new address[](count);
        uint256 amount;
        for (uint256 i; i < count; ++i) {
            beneficiaries[i] = _ratingBeneficiary(clipIds[i], scores[i]);
            // Mark during validation so a repeated clip in this same batch is
            // rejected in O(n). A later failure rolls every mark back.
            hasRated[clipIds[i]][msg.sender] = true;
            amount += uint256(scores[i]) * UNIT;
        }
        uint256 balance = balanceOf(msg.sender);
        if (balance < amount) revert ERC20InsufficientBalance(msg.sender, balance, amount);
        settledSession[msg.sender][sessionId] = true;
        for (uint256 i; i < count; ++i) {
            _payRating(clipIds[i], scores[i], beneficiaries[i], peerCommitment);
        }
        emit RatingsSettled(sessionId, msg.sender, count, amount, peerCommitment);
    }

    function _ratingBeneficiary(bytes32 clipId, uint8 score) private view returns (address beneficiary) {
        beneficiary = clipBeneficiary[clipId];
        if (beneficiary == address(0) || beneficiary == msg.sender || score == 0 || score > 9) {
            revert InvalidRating();
        }
        if (hasRated[clipId][msg.sender]) revert AlreadyRated();
    }

    function _payRating(bytes32 clipId, uint8 score, address beneficiary, bytes32 peerCommitment) private {
        uint256 amount = uint256(score) * UNIT;
        _transfer(msg.sender, beneficiary, amount);
        emit ClipRated(clipId, msg.sender, score, amount, peerCommitment);
    }

    /// @dev Reject the incomplete modulo bucket. Hash expansion is deterministic
    /// and does not expose a caller-controlled retry or a replacement beacon.
    function _uniformTicket(bytes32 entropy) internal pure returns (uint16) {
        uint256 value = uint256(entropy);
        uint256 threshold = addmod(type(uint256).max, 1, 1023);
        uint256 counter;
        while (value < threshold) {
            value = uint256(keccak256(abi.encode(entropy, ++counter)));
        }
        return uint16(value % 1023);
    }

    function _multiplier(uint16 ticket) internal pure returns (uint8) {
        assert(ticket < 1023);
        for (uint8 tier; tier < 10; ++tier) {
            uint16 weight = uint16(512) >> tier;
            if (ticket < weight) return tier;
            ticket -= weight;
        }
        assert(false);
        return 0;
    }
}
