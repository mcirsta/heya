// AVFoundation playback from private memory, confined to the finite media helper.
#import <AVFoundation/AVFoundation.h>
#import <CoreImage/CoreImage.h>
#import <QuartzCore/QuartzCore.h>
#include <math.h>

@interface HeyaVideoBytes : NSObject <AVAssetResourceLoaderDelegate>
@property NSData *bytes;
@end
@implementation HeyaVideoBytes
- (BOOL)resourceLoader:(AVAssetResourceLoader *)loader shouldWaitForLoadingOfRequestedResource:(AVAssetResourceLoadingRequest *)request {
    if (request.contentInformationRequest) {
        request.contentInformationRequest.contentType = @"public.mpeg-4";
        request.contentInformationRequest.contentLength = (long long)self.bytes.length;
        request.contentInformationRequest.byteRangeAccessSupported = YES;
    }
    AVAssetResourceLoadingDataRequest *data = request.dataRequest;
    if (data) {
        long long offset = MAX(data.requestedOffset, data.currentOffset);
        if (offset < 0 || (unsigned long long)offset > self.bytes.length) {
            [request finishLoadingWithError:[NSError errorWithDomain:NSCocoaErrorDomain code:NSFileReadCorruptFileError userInfo:nil]];
            return YES;
        }
        NSUInteger end = data.requestsAllDataToEndOfResource ? self.bytes.length :
            MIN(self.bytes.length, (NSUInteger)data.requestedOffset + (NSUInteger)data.requestedLength);
        for (NSUInteger at = (NSUInteger)offset; at < end;) {
            NSUInteger count = MIN((NSUInteger)(1024 * 1024), end - at);
            [data respondWithData:[self.bytes subdataWithRange:NSMakeRange(at, count)]];
            at += count;
        }
    }
    [request finishLoading];
    return YES;
}
@end

@interface HeyaNativeVideo : NSObject
@property HeyaVideoBytes *memory;
@property AVURLAsset *asset;
@property AVPlayer *player;
@property AVPlayerItemVideoOutput *output;
@property CIContext *context;
@property dispatch_queue_t queue;
@property CGAffineTransform transform;
@property BOOL transformReady;
@property BOOL playing;
@end
@implementation HeyaNativeVideo
- (void)dealloc {
    [_player pause];
    [_player replaceCurrentItemWithPlayerItem:nil];
    [_asset.resourceLoader setDelegate:nil queue:NULL];
}
@end

void *heya_video_new(const uint8_t *bytes, size_t length, const char *uid, float volume, int silent) { @autoreleasepool {
    if (!bytes || !length || length > 100 * 1024 * 1024) return NULL;
    HeyaNativeVideo *state = [HeyaNativeVideo new];
    state.memory = [HeyaVideoBytes new];
    // Rust retains the underlying Vec until this handle has been freed.
    state.memory.bytes = [NSData dataWithBytesNoCopy:(void *)bytes length:length freeWhenDone:NO];
    state.queue = dispatch_queue_create("org.heya.video-memory", DISPATCH_QUEUE_SERIAL);
    state.asset = [AVURLAsset URLAssetWithURL:[NSURL URLWithString:@"heya-memory://video/clip.mp4"] options:nil];
    [state.asset.resourceLoader setDelegate:state.memory queue:state.queue];
    AVPlayerItem *item = [AVPlayerItem playerItemWithAsset:state.asset];
    state.output = [[AVPlayerItemVideoOutput alloc] initWithPixelBufferAttributes:@{
        (id)kCVPixelBufferPixelFormatTypeKey:@(kCVPixelFormatType_32BGRA),
        (id)kCVPixelBufferMetalCompatibilityKey:@YES}];
    [item addOutput:state.output];
    state.player = [AVPlayer playerWithPlayerItem:item];
    state.player.volume = silent ? 0 : MIN(1, MAX(0, volume));
    if (uid && uid[0]) state.player.audioOutputDeviceUniqueID = [NSString stringWithUTF8String:uid];
    state.context = [CIContext contextWithOptions:@{kCIContextCacheIntermediates:@NO}];
    state.transform = CGAffineTransformIdentity;
    state.playing = YES;
    [state.player play];
    return (__bridge_retained void *)state;
} }

int heya_video_control(void *handle, int playing, int muted, double seek) { @autoreleasepool {
    HeyaNativeVideo *state = (__bridge HeyaNativeVideo *)handle;
    if (!state || state.player.status == AVPlayerStatusFailed) return 0;
    state.playing = playing != 0;
    state.player.muted = muted != 0;
    if (isfinite(seek) && seek >= 0 && seek <= 6 * 3600)
        [state.player seekToTime:CMTimeMakeWithSeconds(seek, 600) toleranceBefore:kCMTimeZero toleranceAfter:kCMTimeZero];
    if (playing) [state.player play]; else [state.player pause];
    return 1;
} }

// -1 failed, 0 no new frame, 1 RGBA frame. Native rendering applies orientation
// before fitting into the caller's bounded display buffer.
int heya_video_frame(void *handle, uint8_t *rgba, size_t capacity, uint32_t *width, uint32_t *height,
                     double *position, double *duration, int *phase) { @autoreleasepool {
    HeyaNativeVideo *state = (__bridge HeyaNativeVideo *)handle;
    if (!state || !rgba || !width || !height || !position || !duration || !phase) return -1;
    CFRunLoopRunInMode(kCFRunLoopDefaultMode, 0.001, false);
    AVPlayerItem *item = state.player.currentItem;
    if (item.status == AVPlayerItemStatusFailed || state.player.status == AVPlayerStatusFailed) return -1;
    double at = CMTimeGetSeconds(item.currentTime), total = CMTimeGetSeconds(item.duration);
    *position = isfinite(at) && at >= 0 ? at : 0;
    *duration = isfinite(total) && total >= 0 ? total : 0;
    if (*duration > 6 * 3600 || *position > 6 * 3600) return -1;
    *phase = item.status != AVPlayerItemStatusReadyToPlay ? 1 :
        (*duration > 0 && *position >= *duration - 0.01) ? 0 : state.playing ? 2 : 3;
    if (!state.transformReady && item.status == AVPlayerItemStatusReadyToPlay) {
        AVAssetTrack *track = [state.asset tracksWithMediaType:AVMediaTypeVideo].firstObject;
        if (!track) return -1;
        state.transform = track.preferredTransform;
        state.transformReady = YES;
    }
    CMTime time = [state.output itemTimeForHostTime:CACurrentMediaTime()];
    if (![state.output hasNewPixelBufferForItemTime:time]) return 0;
    CVPixelBufferRef pixel = [state.output copyPixelBufferForItemTime:time itemTimeForDisplay:NULL];
    if (!pixel) return 0;
    CIImage *image = [[CIImage imageWithCVPixelBuffer:pixel] imageByApplyingTransform:state.transform];
    CGRect bounds = image.extent;
    if (!isfinite(bounds.size.width) || !isfinite(bounds.size.height) || bounds.size.width < 1 || bounds.size.height < 1 ||
        bounds.size.width > 16384 || bounds.size.height > 16384) { CVPixelBufferRelease(pixel); return -1; }
    double scale = MIN(1., 960. / MAX(bounds.size.width, bounds.size.height));
    *width = MAX(1, (uint32_t)floor(bounds.size.width * scale));
    *height = MAX(1, (uint32_t)floor(bounds.size.height * scale));
    if ((size_t)*width * *height * 4 > capacity) { CVPixelBufferRelease(pixel); return -1; }
    image = [image imageByApplyingTransform:CGAffineTransformMakeTranslation(-bounds.origin.x, -bounds.origin.y)];
    image = [image imageByApplyingTransform:CGAffineTransformMakeScale(scale, scale)];
    CGColorSpaceRef colors = CGColorSpaceCreateDeviceRGB();
    [state.context render:image toBitmap:rgba rowBytes:*width * 4
                  bounds:CGRectMake(0, 0, *width, *height) format:kCIFormatRGBA8 colorSpace:colors];
    CGColorSpaceRelease(colors);
    CVPixelBufferRelease(pixel);
    return 1;
} }

void heya_video_free(void *handle) { @autoreleasepool {
    if (handle) { HeyaNativeVideo *state = CFBridgingRelease(handle); (void)state; }
} }
