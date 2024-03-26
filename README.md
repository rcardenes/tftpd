Trivial FTP Daemon
==================

No-frills implementation of the no-frills transfer protocol. It doesn't implement
RFC 1350 in full, as it's not meant to support uploads (Write Requests will be met with
an error).

Besides Read Requests, the following RFCs have been implemented:

* Option extension (RFC 2347)
* Block size option (RFC 2348)
* Timeout and Transfer size options (RFC 2349)

The next step will be implementing dynamic file download, based on the client's IP or
MAC address. This is to serve files with different contents to different clients in a
transparent way, without the need for specific paths, prefixes, etc.
