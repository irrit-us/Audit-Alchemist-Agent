# Python, Ruby, PHP, and R

## Python

Prefer built-in `pdb` for a small local reproduction or `debugpy` when an IDE/DAP client is already part of the workflow. `python -m pdb script.py` and `python -m pdb -m package` are interactive. In this harness, supply a bounded command stream that exits, use an existing terminal workflow, or collect a traceback instead. Pdb may restart the program after it exits; do not assume one continue command ends the session.

For IDE attachment, an installed debugpy can run `python -m debugpy --listen 127.0.0.1:5678 --wait-for-client script.py`; start the client in the same supervised session. Use the actual virtual environment. A native extension crash needs native debugger evidence in addition to Python frames. Built-in faulthandler is useful for Python stack diagnostics without a stepping session.

Sources: [pdb](https://docs.python.org/3/library/pdb.html), [debugpy launch options](https://github.com/microsoft/debugpy/wiki/Command-Line-Reference).

## Ruby

Prefer the maintained `debug` gem and its `rdbg` command for supported MRI Ruby versions. In a Bundler project, use `bundle exec rdbg app.rb` when debug is in that bundle; otherwise use the selected Ruby environment's `rdbg`. A terminal or attached debugger client is required for stepping. Inspect breakpoints, exceptions, frames, and local values before adding source instrumentation.

Preserve Bundler dependencies and the real entrypoint. An existing Pry workflow can aid inspection, but verify its stepping integration before treating it as a replacement for rdbg. Native-extension crashes require native tools.

Source: [ruby/debug](https://github.com/ruby/debug).

## PHP

Prefer Xdebug with an IDE or another DBGp client. Confirm the extension is loaded in the actual SAPI using `php --ri xdebug` for CLI PHP; the web/FPM process can use a different configuration. For Xdebug 3 CLI testing with a ready local client, `XDEBUG_MODE=debug php -d xdebug.start_with_request=yes -d xdebug.client_host=127.0.0.1 script.php` enables a connection attempt.

Xdebug connects to the debugger client; merely enabling it does not supply a client. Verify port/configuration for the installed Xdebug major version and set server-to-local path mappings for containers. Do not infer PHP logic failure from a failed debugger connection.

Source: [Xdebug step debugging](https://xdebug.org/docs/step_debug).

## R

Prefer built-in `debugonce(fun)` for a focused function or `browser()` at a deliberate stop; RStudio can provide a graphical interface to this workflow. After an error in an interactive R session, `traceback()` helps locate the failed call chain. Re-run the same data and package environment before stepping.

These are interactive facilities; for unattended Rscript execution, collect error/trace information in the reproduction and exit explicitly rather than waiting at a browser prompt. Lazy evaluation and environments can make a value differ from the caller's apparent expression. Native package crashes require a native debugger.

Source: [R debug/debugonce](https://stat.ethz.ch/R-manual/R-devel/library/base/html/debug.html).
