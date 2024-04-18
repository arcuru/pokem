# Pok'em

This is a (partial) [ntfy.sh](https://ntfy.sh) clone using Matrix.

This runs a Matrix bot account that pings you on Matrix when it's called.

That's it.

It is built using [headjack](https://github.com/arcuru/headjack), a Matrix bot framework in Rust.

## Usage

1. On Matrix, create a room and invite the bot account.
2. Grab the Matrix Room ID from your client.
3. Call `pokem` using the room ID and your desired message.

`pokem` will login to the bot account and send the message to the room given.

## Install

`pokem` is only packaged on crates.io, but it's recommended that you run from git HEAD for now.

For [Nix](https://nixos.org/) users, this repo contains a Nix flake. See the [setup section](#nix) for details on configuring.

## Setup

First, setup an account on any Matrix server for the bot to use.

Create a config file for the bot with its login info.

**IMPORTANT**: Make sure that you setup your allow_list or the bot will not respond to invites

```yaml
homeserver_url: https://matrix.org
username: "pokem"
password: "" # Optional, if not given it will ask for it on first run
allow_list: "" # Regex for allowed accounts.
room_size_limit: 0 # Optional, set a room size limit, will not send notifications to rooms larger than this
state_dir: "$XDG_STATE_HOME/pokem" # Optional, for setting the pokem state directory
```

### Nix

Development is being done using a [Nix flake](https://nixos.wiki/wiki/Flakes).
The easiest way to install pokem is to use nix flakes.

```bash
❯ nix run github:arcuru/pokem
```

The flake contains an [overlay](https://nixos.wiki/wiki/Overlays) to make it easier to import into your own flake config.
To use, add it to your inputs:

```nix
    inputs.pokem.url = "github:arcuru/pokem";
```

And then add the overlay `inputs.pokem.overlays.default` to your pkgs.

The flake also contains a home-manager module for installing pokem as a service.
Import the module into your home-manager config and you can configure `pokem` all from within nix:

```nix
{inputs, ... }: {
  imports = [ inputs.pokem.homeManagerModules.default ];
  services.pokem = {
    enable = true;
    settings = {
        homeserver_url = "https://matrix.jackson.dev";
        username = "pokem";
        password = "hunter2";
        allow_list = "@me:matrix.org|@myfriend:matrix.org";
    };
  };
}
```
