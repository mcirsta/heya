// Native duplex audio, confined to the finite calling helper.
#import <AVFoundation/AVFoundation.h>
#import <AudioToolbox/AudioToolbox.h>
#import <CoreAudio/CoreAudio.h>
#include <math.h>

static AudioDeviceID callDevice(const char *utf8, BOOL input) {
    AudioDeviceID device = kAudioObjectUnknown;
    if (utf8 && utf8[0]) {
        CFStringRef uid = (__bridge CFStringRef)[NSString stringWithUTF8String:utf8];
        AudioValueTranslation value = {&uid, sizeof(uid), &device, sizeof(device)};
        UInt32 size = sizeof(value);
        AudioObjectPropertyAddress address = {kAudioHardwarePropertyTranslateUIDToDevice,
            kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
        if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, NULL, &size, &value) != noErr) return kAudioObjectUnknown;
    } else {
        UInt32 size = sizeof(device);
        AudioObjectPropertyAddress address = {input ? kAudioHardwarePropertyDefaultInputDevice : kAudioHardwarePropertyDefaultOutputDevice,
            kAudioObjectPropertyScopeGlobal, kAudioObjectPropertyElementMain};
        if (AudioObjectGetPropertyData(kAudioObjectSystemObject, &address, 0, NULL, &size, &device) != noErr) return kAudioObjectUnknown;
    }
    return device;
}
@interface HeyaCallAudio : NSObject
@property AVAudioEngine *engine;
@property AVAudioPlayerNode *speaker;
@property AVAudioConverter *converter;
@property AVAudioFormat *pcm;
@property NSLock *lock;
@property NSMutableData *capture;
@property NSUInteger scheduled;
@property float inputGain;
@property BOOL tapped;
@property BOOL failed;
@end
@implementation HeyaCallAudio
- (void)dealloc {
    if (_tapped) [_engine.inputNode removeTapOnBus:0];
    [_speaker stop];
    [_engine stop];
}
@end

void *heya_call_audio_new(const char *input, const char *output, float inputGain, float outputGain, int *error) { @autoreleasepool { @try {
    *error = 7;
    AudioDeviceID microphone = callDevice(input, YES);
    AudioDeviceID speaker = callDevice(output, NO);
    if (microphone == kAudioObjectUnknown) return NULL;
    *error = 5;
    if (speaker == kAudioObjectUnknown) return NULL;
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
    HeyaCallAudio *state = [HeyaCallAudio new];
    state.engine = [AVAudioEngine new];
    state.lock = [NSLock new];
    state.capture = [NSMutableData data];
    state.inputGain = inputGain;
    AVAudioInputNode *source = state.engine.inputNode;
    AVAudioOutputNode *sink = state.engine.outputNode;
    NSError *nativeError = nil;
    // Enabling voice processing can replace the underlying AudioUnit. Select
    // devices afterwards so that transition cannot reset the user's choice.
    if (![source setVoiceProcessingEnabled:YES error:&nativeError]) return NULL;
    // Select actual hardware through Core Audio, without changing system defaults.
    if (AudioUnitSetProperty(source.audioUnit, kAudioOutputUnitProperty_CurrentDevice,
        kAudioUnitScope_Global, 0, &microphone, sizeof(microphone)) != noErr) return NULL;
    if (AudioUnitSetProperty(sink.audioUnit, kAudioOutputUnitProperty_CurrentDevice,
        kAudioUnitScope_Global, 0, &speaker, sizeof(speaker)) != noErr) return NULL;
    AVAudioFormat *hardware = [source outputFormatForBus:0];
    if (hardware.channelCount == 0 || hardware.channelCount > 32 || !isfinite(hardware.sampleRate) ||
        hardware.sampleRate < 8000 || hardware.sampleRate > 192000) return NULL;
    state.pcm = [[AVAudioFormat alloc] initWithCommonFormat:AVAudioPCMFormatFloat32 sampleRate:16000 channels:1 interleaved:NO];
    state.converter = [[AVAudioConverter alloc] initFromFormat:hardware toFormat:state.pcm];
    if (!state.converter) return NULL;
    state.speaker = [AVAudioPlayerNode new];
    state.speaker.volume = outputGain;
    [state.engine attachNode:state.speaker];
    [state.engine connect:state.speaker to:state.engine.mainMixerNode format:state.pcm];
    __weak HeyaCallAudio *weak = state;
    [source installTapOnBus:0 bufferSize:1024 format:hardware block:^(AVAudioPCMBuffer *buffer, AVAudioTime *when) {
        @autoreleasepool {
        (void)when;
        HeyaCallAudio *strong = weak;
        if (!strong || buffer.frameLength == 0) return;
        AVAudioFrameCount capacity = (AVAudioFrameCount)ceil(buffer.frameLength * 16000. / hardware.sampleRate) + 32;
        if (capacity > 8192) { [strong.lock lock]; strong.failed = YES; [strong.lock unlock]; return; }
        AVAudioPCMBuffer *converted = [[AVAudioPCMBuffer alloc] initWithPCMFormat:strong.pcm frameCapacity:capacity];
        __block BOOL supplied = NO;
        NSError *conversionError = nil;
        AVAudioConverterOutputStatus result = [strong.converter convertToBuffer:converted error:&conversionError withInputFromBlock:^AVAudioBuffer *(AVAudioPacketCount count, AVAudioConverterInputStatus *status) {
            (void)count;
            if (supplied) { *status = AVAudioConverterInputStatus_NoDataNow; return nil; }
            supplied = YES; *status = AVAudioConverterInputStatus_HaveData; return buffer;
        }];
        [strong.lock lock];
        if (result == AVAudioConverterOutputStatus_Error) strong.failed = YES;
        else if (converted.frameLength) {
            // Keep at most 200 ms; stale microphone audio must never queue indefinitely.
            NSUInteger bytes = converted.frameLength * sizeof(float);
            if (bytes <= 16000 * 4 / 5) {
                if (strong.capture.length + bytes > 16000 * 4 / 5) strong.capture.length = 0;
                [strong.capture appendBytes:converted.floatChannelData[0] length:bytes];
            }
        }
        [strong.lock unlock];
        }
    }];
    state.tapped = YES;
    if (![state.engine startAndReturnError:&nativeError]) return NULL;
    [state.speaker play];
    *error = 0;
    return (__bridge_retained void *)state;
} @catch (NSException *exception) { (void)exception; *error = 6; return NULL; } } }

int heya_call_audio_read(void *handle, int16_t *pcm, uint32_t count) { @autoreleasepool {
    HeyaCallAudio *state = (__bridge HeyaCallAudio *)handle;
    if (!state || !pcm || count != 960) return -1;
    [state.lock lock];
    if (state.failed || !state.engine.running) { [state.lock unlock]; return -1; }
    if (state.capture.length < count * sizeof(float)) { [state.lock unlock]; return 0; }
    const float *samples = state.capture.bytes;
    for (UInt32 i = 0; i < count; i++) {
        float value = samples[i] * state.inputGain;
        if (!isfinite(value)) { [state.lock unlock]; return -1; }
        pcm[i] = (int16_t)lrintf(fminf(1, fmaxf(-1, value)) * 32767);
    }
    [state.capture replaceBytesInRange:NSMakeRange(0, count * sizeof(float)) withBytes:NULL length:0];
    [state.lock unlock];
    return (int)count;
} }
int heya_call_audio_write(void *handle, const int16_t *pcm, uint32_t count) { @autoreleasepool {
    HeyaCallAudio *state = (__bridge HeyaCallAudio *)handle;
    if (!state || !pcm || count == 0 || count > 4096 || !state.engine.running) return 0;
    [state.lock lock];
    if (state.scheduled >= 3) { [state.lock unlock]; return 1; }
    state.scheduled++;
    [state.lock unlock];
    AVAudioPCMBuffer *buffer = [[AVAudioPCMBuffer alloc] initWithPCMFormat:state.pcm frameCapacity:count];
    buffer.frameLength = count;
    for (UInt32 i = 0; i < count; i++) buffer.floatChannelData[0][i] = (float)pcm[i] / 32768;
    __weak HeyaCallAudio *weak = state;
    [state.speaker scheduleBuffer:buffer completionHandler:^{
        HeyaCallAudio *strong = weak;
        [strong.lock lock]; if (strong.scheduled) strong.scheduled--; [strong.lock unlock];
    }];
    return 1;
} }
void heya_call_audio_free(void *handle) { @autoreleasepool {
    if (handle) { HeyaCallAudio *state = CFBridgingRelease(handle); (void)state; }
} }
