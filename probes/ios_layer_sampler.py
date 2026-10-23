# Attach once, keep running, tap, sample CALayer tree (model + presentation) at intervals.
import sys, time, subprocess
sys.path.insert(0, subprocess.check_output(["lldb", "-P"]).decode().strip())
import lldb

pid, tap_cmd = int(sys.argv[1]), sys.argv[2]
dbg = lldb.SBDebugger.Create()
dbg.SetAsync(False)
t0 = time.time()
target = dbg.CreateTarget("")
err = lldb.SBError()
process = target.AttachToProcessWithID(dbg.GetListener(), pid, err)
print("attach", err.Success(), round(time.time() - t0, 2), "s")

DUMP = r'''
@import UIKit; @import QuartzCore;
NSMutableString *out = [NSMutableString string];
void (^__block walk)(CALayer *, int);
walk = ^(CALayer *l, int depth) {
  CALayer *p = [l presentationLayer] ?: l;
  NSString *pad = [@"" stringByPaddingToLength:depth*2 withString:@" " startingAtIndex:0];
  [out appendFormat:@"%@%@ frame=%@ bg=%@ border=%.2f/%@ corner=%.1f opacity=%.2f mask=%d contents=%@ anims=%@ shadow=%.1f/%.2f extra=%@\n",
    pad, NSStringFromClass([l class]), NSStringFromCGRect(p.frame),
    p.backgroundColor ? [[UIColor colorWithCGColor:p.backgroundColor] description] : @"-",
    p.borderWidth, p.borderColor ? [[UIColor colorWithCGColor:p.borderColor] description] : @"-",
    p.cornerRadius, p.opacity, p.masksToBounds, l.contents ? @"bitmap" : @"-", [l animationKeys], p.shadowRadius, p.shadowOpacity,
    [l respondsToSelector:@selector(colors)] ? (NSString *)[(NSObject *)[l valueForKey:@"colors"] description] : ([l isKindOfClass:[CAShapeLayer class]] ? [NSString stringWithFormat:@"shape fill=%@ stroke=%@ lineWidth=%.2f pathBounds=%@",
      ((CAShapeLayer *)p).fillColor ? [[UIColor colorWithCGColor:((CAShapeLayer *)p).fillColor] description] : @"-",
      ((CAShapeLayer *)p).strokeColor ? [[UIColor colorWithCGColor:((CAShapeLayer *)p).strokeColor] description] : @"-",
      ((CAShapeLayer *)p).lineWidth, NSStringFromCGRect(CGPathGetBoundingBox(((CAShapeLayer *)p).path))] : @"")];
  if (depth < 7) for (CALayer *s in l.sublayers) walk(s, depth + 1);
};
UIWindow *w = [[UIApplication sharedApplication] windows].firstObject;
walk(w.layer, 0);
out;
'''

def dump(tag):
    opts = lldb.SBExpressionOptions()
    opts.SetLanguage(lldb.eLanguageTypeObjC_plus_plus)
    opts.SetTimeoutInMicroSeconds(20_000_000)
    t = time.time()
    v = process.GetSelectedThread().GetSelectedFrame().EvaluateExpression(DUMP, opts)
    print(f"=== {tag} (eval {round(time.time()-t,2)}s)")
    print(v.GetObjectDescription() or v.GetError())

def wait_state(want):
    ev = lldb.SBEvent()
    while process.GetState() != want:
        dbg.GetListener().WaitForEvent(1, ev)

def sample_after(tap_t, at):
    time.sleep(max(0, at - (time.time() - tap_t)))
    process.Stop(); wait_state(lldb.eStateStopped)
    dump(f"t+{round(time.time()-tap_t,2)}s")
    process.Continue(); wait_state(lldb.eStateRunning)

dump("before tap")
dbg.SetAsync(True)
process.Continue(); wait_state(lldb.eStateRunning)
subprocess.run(tap_cmd, shell=True, capture_output=True)
tap_t = time.time()
for at in (0.3, 1.0, 1.7, 3.0):
    sample_after(tap_t, at)
process.Stop(); wait_state(lldb.eStateStopped)
process.Detach()
