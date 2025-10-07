# Design

## Problem Description

The problem is to design a middleware between a quic client and quic server
that breaks long udp connections, which are throttled by my ISP, into multiple short
udp connections.

## General Approach

This is achieved by having a client side middleware that accepts connections from
the client, associate an outgoing port with the client then using that outgoing
port to do port hopping with the server side middleware. (By middleware I really
mean a glorified udp proxy) The server side middleware, in turn, aggregates udp
packets on all of its listening ports. For each sender (ip, port) pair, an
outgoing port is allocated and is used to communicate with the server.

## Detailed Design

Because how simple the logic is, I specifically chose to use low level libraries
to implement both the client and the server as single threaded event loop proactor
applications that utilizie the `sendmsg` and `recvmsg` calls.

Now we just need to describe what the application should do every single event loop.

Note: socket below all contain a buffer

### Client

For client, it initially just listens a single socket, `ds_sock`, and waits for read
readiness events.

#### Event loop

After the events arrive, and before looping over every single event,
we need to do some bookkeeping to update the application state.

#### State update

First we check for each downstream connection, if they time out, we release the
upstream socket by unregistering it at the notifier, releasing its buffer and then
closing the socket. We then update the upstream port we should send packets to
if it needs to be done.

#### Looping over events

If the event is `ds_sock` readable, we use recvmmsg to read all the packets in buffer
and aggregate them by their src (ip, port). For every src (ip, ports), if there's no
connection associated with it, we will open a new socket and register a new connection.
We then reset the connection's timeout timer and write to `us_sock` as much as possible
and put the rest in its buffer.

If the event is `us_sock` readable, we use recvmmsg to read all the packets in the
buffer and filter out all packets that are not from upstream server. We will
also reset the connection's timeout timer and write to `ds_sock` as much as possible
and put the result in its buffer, with dst (ip, port) attached.

If the event is socket writable, we try to empty the buffer using `sendmmsg`.