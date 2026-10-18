// Native audio owns only bounded memory. Capture/encoding run in the finite
// media helper; no microphone is opened by discovery or player construction.
#import <AVFoundation/AVFoundation.h>
#import <AudioToolbox/AudioToolbox.h>
#import <CoreAudio/CoreAudio.h>
#include <math.h>

static const NSUInteger HeyaAudioLimit = 20 * 1024 * 1024;

static UInt32 channelsForDevice(AudioDeviceID device, BOOL input) {
    AudioObjectPropertyAddress address = {kAudioDevicePropertyStreamConfiguration,
        input ? kAudioDevicePropertyScopeInput : kAudioDevicePropertyScopeOutput,
        kAudioObjectPropertyElementMain};
    UInt32 size = 0;
    if (AudioObjectGetPropertyDataSize(device, &address, 0, NULL, &size) != noErr ||
        size < sizeof(AudioBufferList) || size > 65536) return 0;
    AudioBufferList *list = calloc(1, size);
    if (!list) return 0;
    UInt32 channels = 0;
    if (AudioObjectGetPropertyData(device, &address, 0, NULL, &size, list) == noErr) {
        for (UInt32 i = 0; i < list->mNumberBuffers; i++) channels += list->mBuffers[i].mNumberChannels;
    }
    free(list);
    return channels;
}

static NSString *deviceString(AudioDeviceID device, AudioObjectPropertySelector selector) {
    AudioObjectPropertyAddress address = {selector, kAudioObjectPropertyScopeGlobal,
                                          kAudioObjectPropertyElementMain};
    CFStringRef value = NULL;
    UInt32 size = sizeof(value);
    if (AudioObjectGetPropertyData(device, &address, 0, NULL, &size, &value) != noErr) return nil;
    return CFBridgingRelease(value);
}

static NSArray<NSNumber *> *allDevices(void) {
    AudioObjectPropertyAddress address = {kAudioHardwarePropertyDevices,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    UInt32 size = 0;
    if (AudioObjectGetPropertyDataSize(kAudioObjectSystemObject, &address, 0, NULL, &size) != noErr ||
        size > 65536) return nil;
    NSMutableData *data = [NSMutableData dataWithLength:size];
    if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, NULL, &size,
                                  data.mutableBytes) != noErr) return nil;
    NSMutableArray *result = [NSMutableArray array];
    const AudioDeviceID *ids = data.bytes;
    for (NSUInteger i = 0; i < size / sizeof(AudioDeviceID); i++) [result addObject:@(ids[i])];
    return result;
}

static AudioDeviceID findDevice(NSString *uid, BOOL input) {
    if (uid.length) {
        for (NSNumber *number in allDevices()) {
            AudioDeviceID device = number.unsignedIntValue;
            if ([deviceString(device, kAudioDevicePropertyDeviceUID) isEqualToString:uid] &&
                channelsForDevice(device, input) > 0) return device;
        }
        return kAudioObjectUnknown;
    }
    AudioObjectPropertyAddress address = {input ? kAudioHardwarePropertyDefaultInputDevice :
        kAudioHardwarePropertyDefaultOutputDevice, kAudioObjectPropertyScopeGlobal,
        kAudioObjectPropertyElementMain};
    AudioDeviceID device = kAudioObjectUnknown;
    UInt32 size = sizeof(device);
    if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, NULL, &size, &device) != noErr ||
        channelsForDevice(device, input) == 0) return kAudioObjectUnknown;
    return device;
}

static int copyResult(NSData *data, uint8_t **output, size_t *length) {
    if (!data || !output || !length || data.length > HeyaAudioLimit + 2 * 1024 * 1024) return 0;
    *output = malloc(MAX((NSUInteger)1, data.length));
    if (!*output) return 0;
    memcpy(*output, data.bytes, data.length);
    *length = data.length;
    return 1;
}

int heya_audio_devices(uint8_t **output, size_t *length) { @autoreleasepool {
    NSArray *devices = allDevices();
    if (!devices) return 0;
    NSMutableArray *result = [NSMutableArray array];
    for (NSNumber *number in devices) {
        AudioDeviceID device = number.unsignedIntValue;
        NSString *uid = deviceString(device, kAudioDevicePropertyDeviceUID);
        NSString *name = deviceString(device, kAudioObjectPropertyName);
        if (!uid.length || !name.length || [uid lengthOfBytesUsingEncoding:NSUTF8StringEncoding] > 4096) continue;
        for (NSNumber *direction in @[@YES, @NO]) {
            if (channelsForDevice(device, direction.boolValue) && result.count < 128) {
                [result addObject:@{@"id":uid, @"name":name, @"input":direction}];
            }
        }
    }
    NSData *json = [NSJSONSerialization dataWithJSONObject:result options:0 error:NULL];
    return json.length <= 512 * 1024 && copyResult(json, output, length);
} }

@interface HeyaMemoryAudio : NSObject
@property NSMutableData *bytes;
@end
@implementation HeyaMemoryAudio
- (instancetype)init { if ((self = [super init])) _bytes = [NSMutableData data]; return self; }
@end
static OSStatus memoryRead(void *context, SInt64 position, UInt32 count, void *buffer, UInt32 *actual) {
    HeyaMemoryAudio *memory = (__bridge HeyaMemoryAudio *)context;
    if (position < 0 || (UInt64)position > memory.bytes.length) return kAudioFilePositionError;
    *actual = (UInt32)MIN((NSUInteger)count, memory.bytes.length - (NSUInteger)position);
    if (*actual) memcpy(buffer, (const uint8_t *)memory.bytes.bytes + position, *actual);
    return noErr;
}
static OSStatus memoryWrite(void *context, SInt64 position, UInt32 count, const void *buffer, UInt32 *actual) {
    HeyaMemoryAudio *memory = (__bridge HeyaMemoryAudio *)context;
    if (position < 0 || (UInt64)position + count > HeyaAudioLimit + 2 * 1024 * 1024) return kAudioFilePositionError;
    NSUInteger end = (NSUInteger)position + count;
    if (end > memory.bytes.length) memory.bytes.length = end;
    if (count) memcpy((uint8_t *)memory.bytes.mutableBytes + position, buffer, count);
    *actual = count;
    return noErr;
}
static SInt64 memorySize(void *context) { return ((__bridge HeyaMemoryAudio *)context).bytes.length; }
static OSStatus memoryResize(void *context, SInt64 size) {
    if (size < 0 || size > (SInt64)(HeyaAudioLimit + 2 * 1024 * 1024)) return kAudioFilePositionError;
    ((__bridge HeyaMemoryAudio *)context).bytes.length = (NSUInteger)size;
    return noErr;
}

@interface HeyaNativeEncoder : NSObject
@property HeyaMemoryAudio *memory;
@property AudioFileID file;
@property ExtAudioFileRef encoder;
@property UInt32 channels;
@end
@implementation HeyaNativeEncoder
- (void)dealloc {
    if (_encoder) ExtAudioFileDispose(_encoder);
    if (_file) AudioFileClose(_file);
}
@end

@interface HeyaNativePlayer : NSObject
@property HeyaNativeEncoder *reader;
@property AudioQueueRef queue;
@property NSMutableArray<NSValue *> *buffers;
@property NSLock *lock;
@property UInt32 channels;
@property double sourceRate;
@property double duration;
@property double baseSeconds;
@property NSTimeInterval startedAt;
@property BOOL seenRunning;
@property(atomic) BOOL eof;
@property(atomic) BOOL failed;
@property(atomic) BOOL reconfiguring;
@property(atomic) BOOL requestedPlaying;
@end
@implementation HeyaNativePlayer
- (void)dealloc {
    _reconfiguring = YES;
    if (_queue) AudioQueueDispose(_queue, true);
}
@end

static BOOL fillPlaybackBuffer(HeyaNativePlayer *state, AudioQueueBufferRef buffer) {
    [state.lock lock];
    UInt32 frames = buffer->mAudioDataBytesCapacity / (sizeof(float) * state.channels);
    AudioBufferList list = {0}; list.mNumberBuffers = 1;
    list.mBuffers[0].mNumberChannels = state.channels;
    list.mBuffers[0].mDataByteSize = buffer->mAudioDataBytesCapacity;
    list.mBuffers[0].mData = buffer->mAudioData;
    OSStatus result = ExtAudioFileRead(state.reader.encoder, &frames, &list);
    if (result != noErr) state.failed = YES;
    if (frames == 0) state.eof = YES;
    buffer->mAudioDataByteSize = frames * sizeof(float) * state.channels;
    [state.lock unlock];
    if (state.failed || !frames) return NO;
    if (AudioQueueEnqueueBuffer(state.queue, buffer, 0, NULL) != noErr) {
        state.failed = YES; return NO;
    }
    return YES;
}
static void playbackOutput(void *context, AudioQueueRef queue, AudioQueueBufferRef buffer) {
    @autoreleasepool {
        HeyaNativePlayer *state = (__bridge HeyaNativePlayer *)context;
        if (state.reconfiguring) return;
        if (!fillPlaybackBuffer(state, buffer)) AudioQueueStop(queue, false);
    }
}

void *heya_audio_player_new(const uint8_t *bytes, size_t length, const char *uid,
                           float volume, int silent, double *duration, int *error) { @autoreleasepool {
    *error = 1;
    if (!bytes || length == 0 || length > HeyaAudioLimit) return NULL;
    HeyaNativePlayer *state = [HeyaNativePlayer new];
    state.lock = [NSLock new]; state.reader = [HeyaNativeEncoder new];
    state.reader.memory = [HeyaMemoryAudio new];
    state.reader.memory.bytes = [NSMutableData dataWithBytes:bytes length:length];
    AudioFileID file = NULL;
    if (AudioFileOpenWithCallbacks((__bridge void *)state.reader.memory, memoryRead, NULL,
        memorySize, NULL, 0, &file) != noErr) return NULL;
    state.reader.file = file;
    AudioStreamBasicDescription source = {0}; UInt32 size = sizeof(source);
    if (AudioFileGetProperty(file, kAudioFilePropertyDataFormat, &size, &source) != noErr ||
        !isfinite(source.mSampleRate) || source.mSampleRate <= 0 || !source.mChannelsPerFrame) return NULL;
    ExtAudioFileRef decoder = NULL;
    if (ExtAudioFileWrapAudioFileID(file, false, &decoder) != noErr) return NULL;
    state.reader.encoder = decoder; state.sourceRate = source.mSampleRate;
    SInt64 frames = 0; size = sizeof(frames);
    if (ExtAudioFileGetProperty(decoder, kExtAudioFileProperty_FileLengthFrames, &size, &frames) != noErr) return NULL;
    *duration = frames / source.mSampleRate;
    if (!isfinite(*duration) || *duration <= 0 || *duration > 6 * 3600) { *error = 9; return NULL; }
    state.duration = *duration; state.channels = source.mChannelsPerFrame == 1 ? 1 : 2;
    AudioStreamBasicDescription pcm = {0};
    pcm.mSampleRate = 48000; pcm.mFormatID = kAudioFormatLinearPCM;
    pcm.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked;
    pcm.mChannelsPerFrame = state.channels; pcm.mBitsPerChannel = 32;
    pcm.mFramesPerPacket = 1; pcm.mBytesPerFrame = 4 * state.channels; pcm.mBytesPerPacket = pcm.mBytesPerFrame;
    if (ExtAudioFileSetProperty(decoder, kExtAudioFileProperty_ClientDataFormat, sizeof(pcm), &pcm) != noErr) return NULL;
    if (!silent) {
        *error = 5;
        NSString *selection = uid ? [NSString stringWithUTF8String:uid] : @"";
        AudioDeviceID device = findDevice(selection, NO);
        if (device == kAudioObjectUnknown) return NULL;
        AudioQueueRef queue = NULL;
        if (AudioQueueNewOutput(&pcm, playbackOutput, (__bridge void *)state, NULL, NULL, 0, &queue) != noErr) return NULL;
        state.queue = queue;
        CFStringRef deviceUID = CFBridgingRetain(deviceString(device, kAudioDevicePropertyDeviceUID));
        if (!deviceUID) return NULL;
        OSStatus selected = AudioQueueSetProperty(queue, kAudioQueueProperty_CurrentDevice, &deviceUID, sizeof(deviceUID));
        CFRelease(deviceUID);
        UInt32 enabled = 1, bypass = 0;
        if (selected != noErr || AudioQueueSetProperty(queue, kAudioQueueProperty_EnableTimePitch,
            &enabled, sizeof(enabled)) != noErr ||
            AudioQueueSetProperty(queue, kAudioQueueProperty_TimePitchBypass, &bypass, sizeof(bypass)) != noErr ||
            AudioQueueSetParameter(queue, kAudioQueueParam_Volume, fmaxf(0, fminf(1, volume))) != noErr) return NULL;
        state.buffers = [NSMutableArray array];
        for (int i = 0; i < 3; i++) {
            AudioQueueBufferRef buffer = NULL;
            if (AudioQueueAllocateBuffer(queue, 2048 * pcm.mBytesPerFrame, &buffer) != noErr) return NULL;
            [state.buffers addObject:[NSValue valueWithPointer:buffer]];
            if (!state.eof && !fillPlaybackBuffer(state, buffer) && state.failed) return NULL;
        }
    }
    *error = 0;
    return (__bridge_retained void *)state;
} }

int heya_audio_player_control(void *handle, int playing, double position, float speed) { @autoreleasepool {
    HeyaNativePlayer *state = (__bridge HeyaNativePlayer *)handle;
    if (!state || !state.queue || !isfinite(speed) || speed < .5 || speed > 2) return 0;
    if (isfinite(position) && position >= 0) {
        state.reconfiguring = YES;
        if (AudioQueueStop(state.queue, true) != noErr || AudioQueueReset(state.queue) != noErr) return 0;
        [state.lock lock];
        state.baseSeconds = fmin(position, state.duration);
        OSStatus result = ExtAudioFileSeek(state.reader.encoder, (SInt64)(state.baseSeconds * state.sourceRate));
        state.eof = state.baseSeconds >= state.duration; state.failed = result != noErr;
        [state.lock unlock];
        if (state.failed) return 0;
        for (NSValue *value in state.buffers) {
            if (!state.eof && !fillPlaybackBuffer(state, value.pointerValue) && state.failed) return 0;
        }
        state.reconfiguring = NO;
    }
    if (AudioQueueSetParameter(state.queue, kAudioQueueParam_PlayRate, speed) != noErr) return 0;
    if (playing && (!state.requestedPlaying || position >= 0)) {
        state.startedAt = NSProcessInfo.processInfo.systemUptime; state.seenRunning = NO;
    }
    state.requestedPlaying = playing;
    if (state.eof && state.baseSeconds >= state.duration) return 1;
    return (playing ? AudioQueueStart(state.queue, NULL) : AudioQueuePause(state.queue)) == noErr;
} }

int heya_audio_player_position(void *handle, double *position) { @autoreleasepool {
    HeyaNativePlayer *state = (__bridge HeyaNativePlayer *)handle;
    AudioTimeStamp time = {0}; Boolean discontinuity = false;
    *position = state.baseSeconds;
    if (AudioQueueGetCurrentTime(state.queue, NULL, &time, &discontinuity) == noErr &&
        (time.mFlags & kAudioTimeStampSampleTimeValid)) *position += fmax(0, time.mSampleTime) / 48000.;
    *position = fmin(*position, state.duration);
    UInt32 running = 0, size = sizeof(running);
    if (AudioQueueGetProperty(state.queue, kAudioQueueProperty_IsRunning, &running, &size) != noErr || state.failed) return -1;
    if (running) state.seenRunning = YES;
    if (state.requestedPlaying && !running &&
        (state.seenRunning || NSProcessInfo.processInfo.systemUptime - state.startedAt > .5)) return state.eof ? 1 : -1;
    return 0;
} }
void heya_audio_player_free(void *handle) { @autoreleasepool {
    HeyaNativePlayer *state = CFBridgingRelease(handle); state.reconfiguring = YES;
    // Dispose while this worker still owns the state. A callback may retain it;
    // relying on dealloc could otherwise dispose the queue on its own callback.
    if (state.queue) { AudioQueueDispose(state.queue, true); state.queue = NULL; }
} }

void *heya_audio_encoder_new(double rate, uint32_t channels) { @autoreleasepool {
    if (!isfinite(rate) || rate < 8000 || rate > 192000 || channels == 0 || channels > 32) return NULL;
    HeyaNativeEncoder *state = [HeyaNativeEncoder new];
    state.memory = [HeyaMemoryAudio new]; state.channels = channels;
    AudioStreamBasicDescription output = {0};
    output.mSampleRate = 48000; output.mFormatID = kAudioFormatOpus;
    output.mChannelsPerFrame = 1; output.mFramesPerPacket = 960;
    UInt32 size = sizeof(output);
    if (AudioFormatGetProperty(kAudioFormatProperty_FormatInfo, 0, NULL, &size, &output) != noErr) return NULL;
    AudioFileID file = NULL;
    if (AudioFileInitializeWithCallbacks((__bridge void *)state.memory, memoryRead, memoryWrite,
        memorySize, memoryResize, kAudioFileCAFType, &output, kAudioFileFlags_EraseFile, &file) != noErr) return NULL;
    state.file = file;
    ExtAudioFileRef encoder = NULL;
    if (ExtAudioFileWrapAudioFileID(file, true, &encoder) != noErr) return NULL;
    state.encoder = encoder;
    AudioStreamBasicDescription input = {0};
    input.mSampleRate = rate; input.mFormatID = kAudioFormatLinearPCM;
    input.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked;
    input.mChannelsPerFrame = channels; input.mBitsPerChannel = 32;
    input.mFramesPerPacket = 1; input.mBytesPerFrame = 4 * channels; input.mBytesPerPacket = 4 * channels;
    if (ExtAudioFileSetProperty(encoder, kExtAudioFileProperty_ClientDataFormat, sizeof(input), &input) != noErr) return NULL;
    AudioConverterRef converter = NULL; size = sizeof(converter);
    if (ExtAudioFileGetProperty(encoder, kExtAudioFileProperty_AudioConverter, &size, &converter) != noErr) return NULL;
    UInt32 bitrate = 32000;
    if (AudioConverterSetProperty(converter, kAudioConverterEncodeBitRate, sizeof(bitrate), &bitrate) != noErr) return NULL;
    return (__bridge_retained void *)state;
} }

int heya_audio_encoder_write(void *handle, const float *samples, uint32_t frames) { @autoreleasepool {
    HeyaNativeEncoder *state = (__bridge HeyaNativeEncoder *)handle;
    if (!state.encoder || !samples || frames > 8192) return 0;
    AudioBufferList list = {0}; list.mNumberBuffers = 1;
    list.mBuffers[0].mNumberChannels = state.channels;
    list.mBuffers[0].mDataByteSize = frames * state.channels * sizeof(float);
    list.mBuffers[0].mData = (void *)samples;
    return ExtAudioFileWrite(state.encoder, frames, &list) == noErr;
} }

int heya_audio_encoder_finish(void *handle, uint8_t **output, size_t *length) { @autoreleasepool {
    HeyaNativeEncoder *state = (__bridge HeyaNativeEncoder *)handle;
    ExtAudioFileRef encoder = state.encoder; state.encoder = NULL;
    if (!encoder || ExtAudioFileDispose(encoder) != noErr) return 0;
    AudioFileID file = state.file; state.file = NULL;
    if (AudioFileClose(file) != noErr) return 0;
    if (AudioFileOpenWithCallbacks((__bridge void *)state.memory, memoryRead, NULL, memorySize,
                                  NULL, kAudioFileCAFType, &file) != noErr) return 0;
    state.file = file;
    AudioFilePacketTableInfo table = {0}; UInt32 size = sizeof(table);
    if (AudioFileGetProperty(file, kAudioFilePropertyPacketTableInfo, &size, &table) != noErr ||
        table.mNumberValidFrames <= 0 || table.mNumberValidFrames > 1800LL * 48000 ||
        table.mPrimingFrames < 0 || table.mPrimingFrames > 65535) return 0;
    UInt64 count = 0; size = sizeof(count);
    if (AudioFileGetProperty(file, kAudioFilePropertyAudioDataPacketCount, &size, &count) != noErr ||
        count == 0 || count > 1800 * 50 + 32) return 0;
    UInt32 maximum = 0; size = sizeof(maximum);
    if (AudioFileGetProperty(file, kAudioFilePropertyMaximumPacketSize, &size, &maximum) != noErr ||
        maximum == 0 || maximum > 65536) return 0;
    NSMutableData *packets = [NSMutableData data];
    UInt64 valid = CFSwapInt64HostToLittle((UInt64)table.mNumberValidFrames);
    UInt32 priming = CFSwapInt32HostToLittle((UInt32)table.mPrimingFrames);
    UInt32 number = CFSwapInt32HostToLittle((UInt32)count);
    [packets appendBytes:&valid length:8]; [packets appendBytes:&priming length:4]; [packets appendBytes:&number length:4];
    NSMutableData *buffer = [NSMutableData dataWithLength:maximum];
    for (UInt64 i = 0; i < count; i++) {
        UInt32 bytes = maximum, one = 1; AudioStreamPacketDescription description = {0};
        if (AudioFileReadPacketData(file, false, &bytes, &description, (SInt64)i, &one, buffer.mutableBytes) != noErr ||
            one != 1 || bytes == 0 || packets.length + bytes + 4 > HeyaAudioLimit) return 0;
        UInt32 packetSize = CFSwapInt32HostToLittle(bytes);
        [packets appendBytes:&packetSize length:4]; [packets appendBytes:buffer.bytes length:bytes];
    }
    return copyResult(packets, output, length);
} }
void heya_audio_encoder_free(void *handle) { @autoreleasepool {
    HeyaNativeEncoder *state = CFBridgingRelease(handle); (void)state;
} }

@interface HeyaNativeCapture : NSObject
@property AudioQueueRef queue;
@property NSLock *lock;
@property NSMutableData *pending;
@property NSUInteger capacity;
@property(atomic) BOOL stopping;
@property BOOL overflow;
@end
@implementation HeyaNativeCapture
- (void)dealloc {
    _stopping = YES;
    if (_queue) AudioQueueDispose(_queue, true);
}
@end
static void captureInput(void *context, AudioQueueRef queue, AudioQueueBufferRef buffer,
                         const AudioTimeStamp *time, UInt32 packets, const AudioStreamPacketDescription *descriptions) {
    @autoreleasepool {
    (void)time; (void)packets; (void)descriptions;
    HeyaNativeCapture *state = (__bridge HeyaNativeCapture *)context;
    if (state.stopping) return;
    [state.lock lock];
    if (state.pending.length + buffer->mAudioDataByteSize <= state.capacity) {
        [state.pending appendBytes:buffer->mAudioData length:buffer->mAudioDataByteSize];
    } else state.overflow = YES;
    [state.lock unlock];
    if (!state.stopping && AudioQueueEnqueueBuffer(queue, buffer, 0, NULL) != noErr) {
        [state.lock lock]; state.overflow = YES; [state.lock unlock];
    }
    }
}

void *heya_audio_capture_new(const char *uid, double *rate, uint32_t *channels, int *error) { @autoreleasepool {
    *error = 7;
    NSString *selection = uid ? [NSString stringWithUTF8String:uid] : @"";
    AudioDeviceID device = findDevice(selection, YES);
    if (device == kAudioObjectUnknown) return NULL;
    *error = 8;
    if (![NSBundle.mainBundle objectForInfoDictionaryKey:@"NSMicrophoneUsageDescription"]) return NULL;
    AVAuthorizationStatus permission = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
    if (permission == AVAuthorizationStatusNotDetermined) {
        dispatch_semaphore_t ready = dispatch_semaphore_create(0);
        [AVCaptureDevice requestAccessForMediaType:AVMediaTypeAudio completionHandler:^(BOOL granted) {
            (void)granted; dispatch_semaphore_signal(ready);
        }];
        if (dispatch_semaphore_wait(ready, dispatch_time(DISPATCH_TIME_NOW, 60LL * NSEC_PER_SEC))) return NULL;
        permission = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeAudio];
    }
    if (permission != AVAuthorizationStatusAuthorized) return NULL;
    *error = 6;
    AudioObjectPropertyAddress address = {kAudioDevicePropertyNominalSampleRate,
        kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
    UInt32 size = sizeof(*rate);
    if (AudioObjectGetPropertyData(device, &address, 0, NULL, &size, rate) != noErr) return NULL;
    *channels = channelsForDevice(device, YES);
    if (!isfinite(*rate) || *rate < 8000 || *rate > 192000 || *channels == 0 || *channels > 32) return NULL;
    HeyaNativeCapture *state = [HeyaNativeCapture new];
    state.lock = [NSLock new]; state.pending = [NSMutableData data];
    state.capacity = MIN((NSUInteger)(*rate * *channels * 4 * 2), 8 * 1024 * 1024);
    AudioStreamBasicDescription format = {0};
    format.mSampleRate = *rate; format.mFormatID = kAudioFormatLinearPCM;
    format.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked;
    format.mChannelsPerFrame = *channels; format.mBitsPerChannel = 32;
    format.mFramesPerPacket = 1; format.mBytesPerFrame = 4 * *channels; format.mBytesPerPacket = format.mBytesPerFrame;
    AudioQueueRef queue = NULL;
    if (AudioQueueNewInput(&format, captureInput, (__bridge void *)state, NULL, NULL, 0, &queue) != noErr) return NULL;
    state.queue = queue;
    CFStringRef deviceUID = CFBridgingRetain(deviceString(device, kAudioDevicePropertyDeviceUID));
    if (!deviceUID) return NULL;
    OSStatus selected = AudioQueueSetProperty(queue, kAudioQueueProperty_CurrentDevice, &deviceUID, sizeof(deviceUID));
    CFRelease(deviceUID);
    if (selected != noErr) return NULL;
    for (int i = 0; i < 3; i++) {
        AudioQueueBufferRef buffer = NULL;
        UInt32 bytes = (UInt32)(*rate / 50) * format.mBytesPerFrame;
        if (AudioQueueAllocateBuffer(queue, bytes, &buffer) != noErr || AudioQueueEnqueueBuffer(queue, buffer, 0, NULL) != noErr) return NULL;
    }
    // Rust starts capture only after the native encoder is ready.
    *error = 0;
    return (__bridge_retained void *)state;
} }
int heya_audio_capture_control(void *handle, int command) { @autoreleasepool {
    HeyaNativeCapture *state = (__bridge HeyaNativeCapture *)handle;
    if (command == 2) { state.stopping = YES; return AudioQueueStop(state.queue, true) == noErr; }
    return (command == 1 ? AudioQueuePause(state.queue) : AudioQueueStart(state.queue, NULL)) == noErr;
} }
int heya_audio_capture_read(void *handle, float *samples, uint32_t capacity, uint32_t *count) { @autoreleasepool {
    HeyaNativeCapture *state = (__bridge HeyaNativeCapture *)handle;
    [state.lock lock];
    NSUInteger bytes = MIN((NSUInteger)capacity * sizeof(float), state.pending.length);
    memcpy(samples, state.pending.bytes, bytes);
    [state.pending replaceBytesInRange:NSMakeRange(0, bytes) withBytes:NULL length:0];
    *count = (UInt32)(bytes / sizeof(float));
    BOOL ok = !state.overflow;
    [state.lock unlock];
    return ok;
} }
void heya_audio_capture_free(void *handle) { @autoreleasepool {
    HeyaNativeCapture *state = CFBridgingRelease(handle); state.stopping = YES;
    if (state.queue) { AudioQueueDispose(state.queue, true); state.queue = NULL; }
} }

// Offline codec validation; used only by Rust's synthetic tests. It never opens
// an output device and is discarded from the linked release app when unused.
int heya_audio_decode_test(const uint8_t *bytes, size_t length, uint64_t *frames, float *peak) { @autoreleasepool {
    if (!bytes || length == 0 || length > HeyaAudioLimit) return 0;
    HeyaNativeEncoder *state = [HeyaNativeEncoder new];
    state.memory = [HeyaMemoryAudio new];
    state.memory.bytes = [NSMutableData dataWithBytes:bytes length:length];
    AudioFileID file = NULL;
    if (AudioFileOpenWithCallbacks((__bridge void *)state.memory, memoryRead, NULL, memorySize,
                                  NULL, 0, &file) != noErr) return 0;
    state.file = file;
    ExtAudioFileRef decoder = NULL;
    if (ExtAudioFileWrapAudioFileID(file, false, &decoder) != noErr) return 0;
    state.encoder = decoder;
    AudioStreamBasicDescription pcm = {0};
    pcm.mSampleRate = 48000; pcm.mFormatID = kAudioFormatLinearPCM;
    pcm.mFormatFlags = kAudioFormatFlagIsFloat | kAudioFormatFlagIsPacked;
    pcm.mChannelsPerFrame = 1; pcm.mBitsPerChannel = 32;
    pcm.mFramesPerPacket = 1; pcm.mBytesPerFrame = 4; pcm.mBytesPerPacket = 4;
    if (ExtAudioFileSetProperty(decoder, kExtAudioFileProperty_ClientDataFormat, sizeof(pcm), &pcm) != noErr) return 0;
    *frames = 0; *peak = 0;
    for (;;) {
        float samples[4096]; UInt32 count = 4096;
        AudioBufferList list = {0}; list.mNumberBuffers = 1;
        list.mBuffers[0].mNumberChannels = 1; list.mBuffers[0].mDataByteSize = sizeof(samples);
        list.mBuffers[0].mData = samples;
        if (ExtAudioFileRead(decoder, &count, &list) != noErr) return 0;
        if (!count) break;
        *frames += count;
        if (*frames > 10 * 48000) return 0;
        for (UInt32 i = 0; i < count; i++) *peak = fmaxf(*peak, fabsf(samples[i]));
    }
    return 1;
} }
