# Changelog

All notable changes to this project will be documented in this file.

The format is based on [Keep a Changelog](https://keepachangelog.com/en/1.0.0/).

## [Unreleased]

### Added

1. Added support for NVMe-MI responder, gated on a new `nvme-mi` cargo feature

2. Added support for a PLDM for File Transfer requester, triggered on MCTP
   address assignment. This performs a PLDM PDR query to retrieve the
   File Identifier to transfer.

### Changed

1. The log levels for some of the verbose transfer message has been adjusted,
   so we're not as noisy during normal operation

## [0.1] - 2025-06-09
