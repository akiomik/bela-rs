# Connecting the board over Ethernet

Everything in [cross-compile.md](cross-compile.md) reaches the board as
`root@bela.local` over the USB gadget network the stock image sets up.
A USB Ethernet adapter is an alternative path, and one way to give the
board an internet connection of its own. (The image also supports
Wi-Fi, which is out of scope here.)

Verified on a Bela Gem Stereo (PocketBeagle 2), "Bela Debian Bookworm
Image 2026-03-25", with a Realtek RTL8156 adapter (`0bda:8156`, in-tree
`r8152` driver), on 2026-08-05.

## What it buys you, and what it does not

It buys the board its own route to the internet — `apt`, on-board
`git`, IDE updates — and it frees the host from being the board's
uplink.

**It is not a transfer speedup.** That is the intuitive assumption, so
it was measured rather than asserted. Board-to-host, 128 MiB of
incompressible data:

| Path                | raw TCP (iperf3) | over ssh               |
|---------------------|------------------|------------------------|
| USB gadget (`usb1`) | 347 Mbit/s       | 28.5 MB/s (228 Mbit/s) |
| Ethernet adapter    | 194 Mbit/s       | 23.3 MB/s (186 Mbit/s) |

And the step that actually matters, a full `scripts/sync-sysroot.sh`
into an empty destination (~840 MB, three runs each):

| Path                         | elapsed |
|------------------------------|---------|
| USB gadget                   | 40–43 s |
| Ethernet adapter, Wi-Fi host | 39–40 s |
| Ethernet adapter, wired host | 43–47 s |

They all land within a few seconds of each other, for two independent
reasons:

- **The adapter cannot use the gigabit link it negotiates.** It
  enumerates as a high-speed (480 Mbit/s) USB device — `dmesg` reports
  `xhci-hcd: USB3 root hub has no ports` — and board-to-host tops out
  at 194 Mbit/s in practice, short of what the gadget link reaches
  over the same generation of hardware.
- **The sysroot sync is bound by the board's own storage, not by the
  link.** Reading the same tree locally on the board — `tar` to
  `/dev/null`, cold cache, no network involved at all — accounts for
  **25 s** of the 40. It is 8213 files, and walking them holds the
  effective read rate to 34 MB/s where a single large file reads at
  86 MB/s. A faster link cannot buy back much of that.

  Something else dominated until recently: `rsync -z`, whose gzip runs
  on the board's Cortex-A53. The same complete sync with `-z` takes
  **163 s**, which is why `scripts/sync-sysroot.sh` no longer passes
  it.

A better host link does not rescue the Ethernet row. Repeating the
throughput measurements with the host on wired gigabit rather than
Wi-Fi leaves board-to-host at the same 194 Mbit/s, and ssh at
22.4 MB/s. Only the opposite direction gains — 80 Mbit/s to
225 Mbit/s — and this workflow spends almost all of its bytes reading
from the board, not writing to it. The board's CPU is not saturated
while that runs, so the limit is neither the LAN, nor the host, nor
the CPU: the adapter's USB host path simply does not reach what the
gadget link does over the same generation of hardware.

## What the stock image does

`systemd-networkd` manages the network, and the image ships units for
the gadget and wireless interfaces only:

```
/etc/systemd/network/{usb0,usb1,wlan0,mlan0,SoftAp0}.network
```

`usb0` is `192.168.6.2/24` and `usb1` is `192.168.7.2/24`; both run a
DHCP server for the host and a DHCP client of their own.

There is no unit for a wired interface. `systemd` only ships
`/lib/systemd/network/80-ethernet.network.example`, which is not
active.

## Diagnosing

Plug the adapter in and check that the kernel bound a driver:

```sh
lsusb
dmesg | grep -iE 'usb|eth'
```

Then check what `networkd` did with it:

```sh
networkctl list
```

If the driver is bound but the link shows up as

```
IDX LINK   TYPE     OPERATIONAL SETUP
  5 eth0   ether    off         unmanaged
```

then the adapter is fine and only the missing unit is the problem. A
link that never appears at all is a driver problem instead, and
nothing below will help.

## The configuration

Create `/etc/systemd/network/20-eth0.network`:

```ini
[Match]
Name=eth0

[Link]
RequiredForOnline=no

[Network]
DHCP=yes
IPv6AcceptRA=yes

[DHCPv4]
RouteMetric=100
UseDomains=yes
```

- `Name=eth0` — the kernel command line carries `net.ifnames=0`, so
  interfaces keep kernel-style names and any USB NIC becomes `eth0`.
  Matching the name rather than a MAC address survives swapping the
  adapter.
- `RouteMetric=100` — `usb0` and `usb1` are DHCP clients too, so they
  can install a default route of their own at the default metric of
  1024. The lower number keeps Ethernet preferred.
- `RequiredForOnline=no` — otherwise booting without a cable stalls
  `systemd-networkd-wait-online`.

Apply it without a reboot:

```sh
networkctl reload
```

## Verifying

```sh
networkctl list                 # eth0: routable / configured
ip -br addr show eth0
ip route                        # default via ... dev eth0 metric 100
resolvectl query deb.debian.org
```

The unit lives in `/etc/systemd/network`, so it survives reboots.

## Gotcha: the board may rename itself to `bela-2.local`

Hot-plugging the adapter *after* `avahi-daemon` has already claimed
`bela.local` over the gadget link can trigger a name conflict. Avahi
renames itself and does not retry the preferred name:

```
avahi-daemon[352]: Host name conflict, retrying with bela-2
avahi-daemon[352]: Server startup complete. Host name is bela-2.local.
```

The symptom is delayed, because the host's mDNS cache keeps answering
for a while. Once it expires, every `root@bela.local` command in this
repository fails to resolve — `scp`, `ssh`, `scripts/sync-sysroot.sh`
and `scripts/update-vendor.sh` alike.

Recover with:

```sh
systemctl restart avahi-daemon
```

Booting with the adapter already attached does not hit this: `eth0` is
configured before `avahi-daemon` claims the name.

## One name, several addresses

With both the adapter and the USB cable connected, `bela.local`
resolves to the gadget and LAN addresses at once (IPv4 and IPv6 for
each). That is one host with several interfaces, not a conflict, and
which one a client picks is up to it.

Both helper scripts take an explicit host as their second argument if
you want to pin the path:

```sh
scripts/sync-sysroot.sh bela-sysroot root@192.168.11.11
```
