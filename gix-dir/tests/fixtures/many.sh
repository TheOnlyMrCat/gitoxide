#!/bin/bash
set -eu -o pipefail


git init breakout-symlink
(cd breakout-symlink
  mkdir hide
  ln -s ../.. hide/breakout
)

ln -s breakout-symlink symlink-to-breakout-symlink

git init immediate-breakout-symlink
(cd immediate-breakout-symlink
  ln -s .. breakout
)
