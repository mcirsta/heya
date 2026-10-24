// Native call camera/H.264 codecs. All objects live in the finite video helper.
#import <AVFoundation/AVFoundation.h>
#import <VideoToolbox/VideoToolbox.h>
#import <CoreImage/CoreImage.h>
#import <QuartzCore/QuartzCore.h>
#include <math.h>
#include <stdio.h>

@interface HeyaCallVideo : NSObject <AVCaptureVideoDataOutputSampleBufferDelegate> {
@public
    VTCompressionSessionRef encoder;
    VTDecompressionSessionRef decoder;
    CMVideoFormatDescriptionRef format;
    CVPixelBufferRef localFrame;
    CVPixelBufferRef remoteFrame;
}
@property AVCaptureSession *capture;
@property AVCaptureVideoDataOutput *output;
@property dispatch_queue_t queue;
@property CIContext *context;
@property NSLock *lock;
@property NSData *encoded;
@property NSData *sps;
@property NSData *pps;
@property uint32_t timestamp;
@property BOOL localDirty;
@property BOOL remoteDirty;
@property BOOL synthetic;
@property BOOL cameraOn;
@property BOOL forceKeyframe;
@property BOOL needKeyframe;
@property BOOL failed;
@property double lastCapture;
@property double lastSynthetic;
- (void)encode:(CVPixelBufferRef)pixel time:(CMTime)time;
@end

static OSStatus encoderProperty(HeyaCallVideo *state, CFStringRef key, CFTypeRef value) {
    OSStatus status = VTSessionSetProperty(state->encoder, key, value);
    if (status && state.synthetic) {
        char name[160] = {0};
        CFStringGetCString(key, name, sizeof(name), kCFStringEncodingUTF8);
        fprintf(stderr, "Heya synthetic H.264 property %s failed (%d)\n", name, (int)status);
    }
    return status;
}

static void encodedVideo(void *ref, void *source, OSStatus status,
                         VTEncodeInfoFlags flags, CMSampleBufferRef sample) {
    (void)source; (void)flags;
    HeyaCallVideo *state = (__bridge HeyaCallVideo *)ref;
    if (!state.cameraOn) return;
    if (status || !sample || !CMSampleBufferDataIsReady(sample)) {
        if (state.synthetic) fprintf(stderr, "Heya synthetic H.264 encode failed (%d)\n", (int)status);
        state.failed = YES; return;
    }
    CMBlockBufferRef block = CMSampleBufferGetDataBuffer(sample);
    size_t length = block ? CMBlockBufferGetDataLength(block) : 0;
    if (!length || length > 2 * 1024 * 1024) {state.failed = YES; return;}
    NSMutableData *avcc = [NSMutableData dataWithLength:length];
    if (CMBlockBufferCopyDataBytes(block, 0, length, avcc.mutableBytes)) {state.failed = YES; return;}
    CFArrayRef attachments = CMSampleBufferGetSampleAttachmentsArray(sample, false);
    BOOL keyframe = !attachments || !CFArrayGetCount(attachments) ||
        !CFDictionaryContainsKey(CFArrayGetValueAtIndex(attachments, 0), kCMSampleAttachmentKey_NotSync);
    NSMutableData *annex = [NSMutableData data];
    const uint8_t prefix[] = {0, 0, 0, 1};
    if (keyframe) {
        CMFormatDescriptionRef description = CMSampleBufferGetFormatDescription(sample);
        for (size_t i = 0; i < 2; ++i) {
            const uint8_t *data = NULL; size_t size = 0;
            if (CMVideoFormatDescriptionGetH264ParameterSetAtIndex(description, i, &data, &size, NULL, NULL) || !size || size > 65536) {state.failed = YES; return;}
            [annex appendBytes:prefix length:4]; [annex appendBytes:data length:size];
        }
    }
    const uint8_t *bytes = avcc.bytes;
    for (size_t at = 0; at < length;) {
        if (length - at < 4) {state.failed = YES; return;}
        uint32_t size = ((uint32_t)bytes[at] << 24) | ((uint32_t)bytes[at+1] << 16) |
            ((uint32_t)bytes[at+2] << 8) | bytes[at+3];
        at += 4;
        if (!size || size > length - at || annex.length + size + 4 > 2 * 1024 * 1024) {state.failed = YES; return;}
        [annex appendBytes:prefix length:4]; [annex appendBytes:bytes + at length:size]; at += size;
    }
    double seconds = CMTimeGetSeconds(CMSampleBufferGetPresentationTimeStamp(sample));
    uint32_t stamp = isfinite(seconds) && seconds >= 0 ? (uint32_t)fmod(seconds * 90000., 4294967296.) : 0;
    [state.lock lock]; state.encoded = annex; state.timestamp = stamp; [state.lock unlock];
}
static void decodedVideo(void *ref, void *source, OSStatus status,
                         VTDecodeInfoFlags flags, CVImageBufferRef image,
                         CMTime time, CMTime duration) {
    (void)source; (void)flags; (void)time; (void)duration;
    HeyaCallVideo *state = (__bridge HeyaCallVideo *)ref;
    if (status || !image) { state.needKeyframe = YES; return; }
    [state.lock lock];
    if (state->remoteFrame) CVPixelBufferRelease(state->remoteFrame);
    state->remoteFrame = CVPixelBufferRetain(image); state.remoteDirty = YES;
    [state.lock unlock];
}
@implementation HeyaCallVideo
- (void)encode:(CVPixelBufferRef)pixel time:(CMTime)time {
    if (!self.cameraOn) return;
    if (!encoder) {
        size_t width = CVPixelBufferGetWidth(pixel), height = CVPixelBufferGetHeight(pixel);
        if (!width || !height || width > 960 || height > 960) {self.failed = YES; return;}
        OSStatus created = VTCompressionSessionCreate(NULL, (int32_t)width, (int32_t)height,
                kCMVideoCodecType_H264, NULL, NULL, NULL, encodedVideo,
                (__bridge void *)self, &encoder);
        if (created) {
            if (self.synthetic) fprintf(stderr, "Heya synthetic H.264 create failed (%d)\n", (int)created);
            self.failed = YES; return;
        }
        if (encoderProperty(self, kVTCompressionPropertyKey_RealTime, kCFBooleanTrue)) {self.failed = YES; return;}
        // Some Apple encoders do not expose this optional latency control.
        // Readiness still requires an actual encoded and decoded frame.
        (void)encoderProperty(self, kVTCompressionPropertyKey_MaxFrameDelayCount, (__bridge CFTypeRef)@0);
        if (encoderProperty(self, kVTCompressionPropertyKey_AllowFrameReordering, kCFBooleanFalse) ||
            encoderProperty(self, kVTCompressionPropertyKey_ProfileLevel, kVTProfileLevel_H264_Baseline_AutoLevel) ||
            encoderProperty(self, kVTCompressionPropertyKey_AverageBitRate, (__bridge CFTypeRef)@700000) ||
            encoderProperty(self, kVTCompressionPropertyKey_MaxKeyFrameInterval, (__bridge CFTypeRef)@15) ||
            encoderProperty(self, kVTCompressionPropertyKey_ExpectedFrameRate, (__bridge CFTypeRef)@15)) {self.failed = YES; return;}
        OSStatus prepared = VTCompressionSessionPrepareToEncodeFrames(encoder);
        if (prepared) {
            if (self.synthetic) fprintf(stderr, "Heya synthetic H.264 prepare failed (%d)\n", (int)prepared);
            self.failed = YES; return;
        }
    }
    [self.lock lock];
    if (localFrame) CVPixelBufferRelease(localFrame);
    localFrame = CVPixelBufferRetain(pixel); self.localDirty = YES;
    [self.lock unlock];
    NSDictionary *options = self.forceKeyframe ? @{(__bridge NSString *)kVTEncodeFrameOptionKey_ForceKeyFrame:@YES} : nil;
    self.forceKeyframe = NO;
    if (VTCompressionSessionEncodeFrame(encoder, pixel, time, CMTimeMake(1,15),
        (__bridge CFDictionaryRef)options, NULL, NULL)) self.failed = YES;
}
- (void)captureOutput:(AVCaptureOutput *)output didOutputSampleBuffer:(CMSampleBufferRef)sample fromConnection:(AVCaptureConnection *)connection {
    @autoreleasepool {
    (void)output; (void)connection;
    CMTime time = CMSampleBufferGetPresentationTimeStamp(sample);
    double seconds = CMTimeGetSeconds(time);
    if (!self.cameraOn || !isfinite(seconds) || seconds - self.lastCapture < 1. / 15.) return;
    self.lastCapture = seconds;
    CVPixelBufferRef pixel = CMSampleBufferGetImageBuffer(sample);
    if (pixel) [self encode:pixel time:time];
    }
}
- (void)dealloc {
    [_output setSampleBufferDelegate:nil queue:NULL];
    [_capture stopRunning];
    if (_queue) dispatch_sync(_queue, ^{});
    if (encoder) {VTCompressionSessionCompleteFrames(encoder, kCMTimeInvalid); VTCompressionSessionInvalidate(encoder); CFRelease(encoder);}
    if (decoder) {VTDecompressionSessionWaitForAsynchronousFrames(decoder); VTDecompressionSessionInvalidate(decoder); CFRelease(decoder);}
    if (format) CFRelease(format);
    if (localFrame) CVPixelBufferRelease(localFrame);
    if (remoteFrame) CVPixelBufferRelease(remoteFrame);
}
@end

void *heya_call_video_new(int synthetic, int *error) { @autoreleasepool { @try {
    *error = 4;
    HeyaCallVideo *state = [HeyaCallVideo new];
    state.synthetic = synthetic != 0; state.cameraOn = YES; state.forceKeyframe = YES;
    state.lock = [NSLock new]; state.context = [CIContext contextWithOptions:@{kCIContextCacheIntermediates:@NO}];
    if (state.synthetic) return (__bridge_retained void *)state;
    *error = 1;
    AVCaptureDevice *camera = [AVCaptureDevice defaultDeviceWithMediaType:AVMediaTypeVideo];
    if (!camera) return NULL;
    *error = 2;
    if (![NSBundle.mainBundle objectForInfoDictionaryKey:@"NSCameraUsageDescription"]) return NULL;
    AVAuthorizationStatus permission = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeVideo];
    if (permission == AVAuthorizationStatusNotDetermined) {
        dispatch_semaphore_t ready = dispatch_semaphore_create(0);
        [AVCaptureDevice requestAccessForMediaType:AVMediaTypeVideo completionHandler:^(BOOL granted) {(void)granted; dispatch_semaphore_signal(ready);}];
        if (dispatch_semaphore_wait(ready, dispatch_time(DISPATCH_TIME_NOW, 60LL * NSEC_PER_SEC))) return NULL;
        permission = [AVCaptureDevice authorizationStatusForMediaType:AVMediaTypeVideo];
    }
    if (permission != AVAuthorizationStatusAuthorized) return NULL;
    *error = 4;
    NSError *failure = nil;
    AVCaptureDeviceInput *input = [AVCaptureDeviceInput deviceInputWithDevice:camera error:&failure];
    if (!input) return NULL;
    state.capture = [AVCaptureSession new];
    if (![state.capture canSetSessionPreset:AVCaptureSessionPreset640x480]) return NULL;
    state.capture.sessionPreset = AVCaptureSessionPreset640x480;
    if (![state.capture canAddInput:input]) return NULL;
    [state.capture addInput:input];
    state.output = [AVCaptureVideoDataOutput new];
    state.output.videoSettings = @{(id)kCVPixelBufferPixelFormatTypeKey:@(kCVPixelFormatType_32BGRA)};
    state.output.alwaysDiscardsLateVideoFrames = YES;
    if (![state.capture canAddOutput:state.output]) return NULL;
    [state.capture addOutput:state.output];
    state.queue = dispatch_queue_create("org.heya.call-camera", DISPATCH_QUEUE_SERIAL);
    [state.output setSampleBufferDelegate:state queue:state.queue];
    [state.capture startRunning];
    if (!state.capture.running) return NULL;
    return (__bridge_retained void *)state;
} @catch (NSException *exception) {(void)exception; *error = 4; return NULL;} } }

static NSArray<NSData *> *annexUnits(const uint8_t *bytes, size_t length) {
    NSMutableArray<NSData *> *units = [NSMutableArray array];
    size_t begin = SIZE_MAX;
    for (size_t at = 0; at + 3 <= length;) {
        size_t prefix = 0;
        if (at + 4 <= length && !bytes[at] && !bytes[at+1] && !bytes[at+2] && bytes[at+3] == 1) prefix = 4;
        else if (!bytes[at] && !bytes[at+1] && bytes[at+2] == 1) prefix = 3;
        if (prefix) {
            if (begin != SIZE_MAX && at > begin) [units addObject:[NSData dataWithBytes:bytes + begin length:at - begin]];
            begin = at + prefix; at += prefix;
        } else ++at;
    }
    if (begin != SIZE_MAX && length > begin) [units addObject:[NSData dataWithBytes:bytes + begin length:length - begin]];
    return units;
}
int heya_call_video_receive(void *handle, const uint8_t *bytes, size_t length) { @autoreleasepool { @try {
    if (!handle || !bytes || !length || length > 2 * 1024 * 1024) return 0;
    HeyaCallVideo *state = (__bridge HeyaCallVideo *)handle;
    NSArray<NSData *> *units = annexUnits(bytes, length);
    BOOL changed = NO;
    for (NSData *unit in units) {
        unsigned type = ((const uint8_t *)unit.bytes)[0] & 31;
        if (type == 7 && ![state.sps isEqualToData:unit]) {state.sps = unit; changed = YES;}
        if (type == 8 && ![state.pps isEqualToData:unit]) {state.pps = unit; changed = YES;}
    }
    if ((changed || !state->decoder) && state.sps && state.pps) {
        if (state->decoder) {VTDecompressionSessionWaitForAsynchronousFrames(state->decoder); VTDecompressionSessionInvalidate(state->decoder); CFRelease(state->decoder); state->decoder = NULL;}
        if (state->format) {CFRelease(state->format); state->format = NULL;}
        const uint8_t *sets[] = {state.sps.bytes, state.pps.bytes};
        size_t sizes[] = {state.sps.length, state.pps.length};
        if (CMVideoFormatDescriptionCreateFromH264ParameterSets(NULL, 2, sets, sizes, 4, &state->format)) {state.needKeyframe = YES; return 1;}
        CMVideoDimensions dimensions = CMVideoFormatDescriptionGetDimensions(state->format);
        if (dimensions.width <= 0 || dimensions.height <= 0 || dimensions.width > 4096 || dimensions.height > 4096) return 0;
        NSDictionary *attributes = @{(id)kCVPixelBufferPixelFormatTypeKey:@(kCVPixelFormatType_32BGRA), (id)kCVPixelBufferIOSurfacePropertiesKey:@{}};
        VTDecompressionOutputCallbackRecord callback = {decodedVideo, (__bridge void *)state};
        if (VTDecompressionSessionCreate(NULL, state->format, NULL, (__bridge CFDictionaryRef)attributes, &callback, &state->decoder)) return 0;
    }
    if (!state->decoder) {state.needKeyframe = YES; return 1;}
    NSMutableData *avcc = [NSMutableData data];
    for (NSData *unit in units) {
        unsigned type = ((const uint8_t *)unit.bytes)[0] & 31;
        if (type != 1 && type != 5 && type != 6 && type != 9) continue;
        uint32_t size = CFSwapInt32HostToBig((uint32_t)unit.length);
        [avcc appendBytes:&size length:4]; [avcc appendData:unit];
    }
    if (!avcc.length) return 1;
    CMBlockBufferRef block = NULL;
    if (CMBlockBufferCreateWithMemoryBlock(NULL, NULL, avcc.length, NULL, NULL, 0, avcc.length, 0, &block)) return 0;
    OSStatus copied = CMBlockBufferReplaceDataBytes(avcc.bytes, block, 0, avcc.length);
    CMSampleBufferRef sample = NULL;
    size_t size = avcc.length;
    OSStatus made = copied ? copied : CMSampleBufferCreateReady(NULL, block, state->format, 1, 0, NULL, 1, &size, &sample);
    CFRelease(block);
    if (made) return 0;
    OSStatus decoded = VTDecompressionSessionDecodeFrame(state->decoder, sample, kVTDecodeFrame_EnableAsynchronousDecompression, NULL, NULL);
    CFRelease(sample);
    if (decoded) state.needKeyframe = YES;
    return 1;
} @catch (NSException *exception) {(void)exception; return 0;} } }

int heya_call_video_poll(void *handle, uint8_t *bytes, size_t capacity,
                         uint32_t *tag, uint32_t *width, uint32_t *height, uint32_t *timestamp) { @autoreleasepool { @try {
    HeyaCallVideo *state = (__bridge HeyaCallVideo *)handle;
    if (state.failed) return -1;
    double now = CACurrentMediaTime();
    if (state.synthetic && state.cameraOn && now - state.lastSynthetic >= 1. / 15.) {
        state.lastSynthetic = now;
        CVPixelBufferRef pixel = NULL;
        if (CVPixelBufferCreate(NULL, 640, 480, kCVPixelFormatType_32BGRA,
            (__bridge CFDictionaryRef)@{(id)kCVPixelBufferIOSurfacePropertiesKey:@{}}, &pixel)) return -1;
        CVPixelBufferLockBaseAddress(pixel, 0);
        uint8_t *base = CVPixelBufferGetBaseAddress(pixel); size_t row = CVPixelBufferGetBytesPerRow(pixel);
        for (size_t y = 0; y < 480; ++y) for (size_t x = 0; x < 640; ++x) {
            uint8_t *p = base + y * row + x * 4; p[0] = 150; p[1] = (uint8_t)(y / 3); p[2] = (uint8_t)(x / 4); p[3] = 255;
        }
        CVPixelBufferUnlockBaseAddress(pixel, 0);
        [state encode:pixel time:CMTimeMakeWithSeconds(now, 90000)]; CVPixelBufferRelease(pixel);
    }
    [state.lock lock];
    if (state.encoded) {
        NSData *encoded = state.encoded; state.encoded = nil;
        *timestamp = state.timestamp; [state.lock unlock];
        if (encoded.length > capacity) return -1;
        memcpy(bytes, encoded.bytes, encoded.length); *tag = 1; return (int)encoded.length;
    }
    CVPixelBufferRef pixel = NULL;
    if (state.localDirty && state->localFrame) {pixel = CVPixelBufferRetain(state->localFrame); state.localDirty = NO; *tag = 2;}
    else if (state.remoteDirty && state->remoteFrame) {pixel = CVPixelBufferRetain(state->remoteFrame); state.remoteDirty = NO; *tag = 3;}
    [state.lock unlock];
    if (!pixel) {if (state.needKeyframe) {state.needKeyframe = NO; *tag = 7; return 1;} return 0;}
    CIImage *image = [CIImage imageWithCVPixelBuffer:pixel];
    CGFloat scale = MIN(1., MIN(960. / image.extent.size.width, 960. / image.extent.size.height));
    image = [image imageByApplyingTransform:CGAffineTransformMakeScale(scale, scale)];
    *width = (uint32_t)floor(image.extent.size.width); *height = (uint32_t)floor(image.extent.size.height);
    size_t length = (size_t)*width * *height * 4;
    if (!*width || !*height || length > capacity) {CVPixelBufferRelease(pixel); return -1;}
    CGColorSpaceRef space = CGColorSpaceCreateDeviceRGB();
    [state.context render:image toBitmap:bytes rowBytes:*width * 4 bounds:CGRectMake(0, 0, *width, *height) format:kCIFormatRGBA8 colorSpace:space];
    CGColorSpaceRelease(space); CVPixelBufferRelease(pixel); return (int)length;
} @catch (NSException *exception) {(void)exception; return -1;} } }

int heya_call_video_camera(void *handle, int enabled) { @autoreleasepool { @try {
    HeyaCallVideo *state = (__bridge HeyaCallVideo *)handle;
    state.cameraOn = enabled != 0;
    if (enabled) {state.forceKeyframe = YES; [state.capture startRunning]; return state.synthetic || state.capture.running;}
    [state.capture stopRunning];
    if (state.queue) dispatch_sync(state.queue, ^{});
    if (state->encoder) VTCompressionSessionCompleteFrames(state->encoder, kCMTimeInvalid);
    [state.lock lock]; state.encoded = nil; state.localDirty = NO;
    if (state->localFrame) {CVPixelBufferRelease(state->localFrame); state->localFrame = NULL;}
    [state.lock unlock]; return 1;
} @catch (NSException *exception) {(void)exception; return 0;} } }
void heya_call_video_keyframe(void *handle) {((__bridge HeyaCallVideo *)handle).forceKeyframe = YES;}
void heya_call_video_free(void *handle) { @autoreleasepool {if (handle) {HeyaCallVideo *state = (__bridge_transfer HeyaCallVideo *)handle; (void)state;}} }
