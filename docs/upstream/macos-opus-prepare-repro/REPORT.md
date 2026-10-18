# AVAudioPlayer.prepareToPlay crashes with SIGFPE for Ogg/Opus

**Environment:** macOS 15.8.1 (24H32), x86_64, iMac19,1;
Apple Clang 17.0.0 (clang-1700.0.13.5).

`AVAudioPlayer` initializes successfully from the attached Ogg/Opus file and
reports a one-second duration and one channel. Calling `prepareToPlay` then
terminates the process with `EXC_ARITHMETIC (SIGFPE)`.

## Steps to reproduce

With Apple's Command Line Tools installed, extract the attachments and run:

```sh
sh build.sh
./opus-prepare-repro tone.ogg
```

The standalone Objective-C program runs on the main thread, sets the player's
volume to zero, and never calls `play`. No microphone access is involved.

**Expected:** prepare playback successfully, or return a failure if this
container/codec combination is unsupported.

**Actual:** initialization succeeds; `prepareToPlay` raises SIGFPE before returning.

```text
AVFAudio  AVAudioPlayerCpp::setPlaybackFramePos(long long, bool) + 60
AVFAudio  AVAudioPlayerCpp::prepareToPlayQueue() + 115
AVFAudio  AVAudioPlayerCpp::DoAction(unsigned int, unsigned long, void const*) + 256
AVFAudio  -[AVAudioPlayer prepareToPlay] + 26
```

## Control and fixture

`./opus-prepare-repro tone.caf` returns `YES` from `prepareToPlay`.
Both files contain the same native-encoded Opus packets: a generated one-second,
440 Hz sine wave, 48 kHz mono, 32 kbit/s. Apple's `afconvert` decodes both files
to identical PCM samples: 48,000 mono frames. No personal recordings are included.

The reproduction was verified on the macOS version above; other versions and
Apple Silicon have not been tested.
