import 'package:flutter/material.dart';
import 'package:flutter/services.dart';
import 'package:flutter_native_splash/flutter_native_splash.dart';
import 'package:get_10101/backend.dart';
import 'package:get_10101/common/scrollable_safe_area.dart';
import 'package:get_10101/common/snack_bar.dart';
import 'package:get_10101/features/stable/stable_screen.dart';
import 'package:get_10101/features/trade/trade_screen.dart';
import 'package:get_10101/features/wallet/wallet_screen.dart';
import 'package:get_10101/features/welcome/new_wallet_screen.dart';
import 'package:get_10101/features/welcome/welcome_screen.dart';
import 'package:get_10101/logger/logger.dart';
import 'package:get_10101/util/preferences.dart';
import 'package:get_10101/util/file.dart';
import 'package:go_router/go_router.dart';

class LoadingScreen extends StatefulWidget {
  static const route = "/loading";

  final Future<void>? restore;

  const LoadingScreen({super.key, this.restore});

  @override
  State<LoadingScreen> createState() => _LoadingScreenState();
}

class _LoadingScreenState extends State<LoadingScreen> {
  String message = "Welcome to 10101";

  @override
  void initState() {
    List<Future<dynamic>> futures = [
      Preferences.instance.hasEmailAddress(),
      Preferences.instance.getOpenPosition(),
      isSeedFilePresent(),
      Preferences.instance.isFullBackupRequired(),
    ];

    if (widget.restore != null) {
      // wait for the restore process to finish!
      futures.add(widget.restore!);
    }

    Future.wait<dynamic>(futures).then((value) {
      final hasEmailAddress = value[0];
      final position = value[1];
      final isSeedFilePresent = value[2];
      final isFullBackupRequired = value[3];

      FlutterNativeSplash.remove();

      if (isSeedFilePresent) {
        if (!hasEmailAddress) {
          GoRouter.of(context).go(WelcomeScreen.route);
        } else {
          if (isFullBackupRequired) {
            setState(() => message = "Creating initial backup!");
            fullBackup().then((value) {
              Preferences.instance.setFullBackupRequired(false).then((value) {
                start(context, position);
              });
            });
          } else {
            start(context, position);
          }
        }
      } else {
        Preferences.instance.setFullBackupRequired(false).then((value) {
          GoRouter.of(context).go(NewWalletScreen.route);
        });
      }
    });
    super.initState();
  }

  void start(BuildContext context, String? position) {
    setState(() => message = "Starting 10101");
    runBackend(context).then((value) {
      logger.i("Backend started");

      switch (position) {
        case StableScreen.label:
          GoRouter.of(context).go(StableScreen.route);
        case TradeScreen.label:
          GoRouter.of(context).go(TradeScreen.route);
        default:
          GoRouter.of(context).go(WalletScreen.route);
      }
    }).catchError((error) {
      logger.e("Failed to start backend. $error");
      showSnackBar(ScaffoldMessenger.of(context), "Failed to start 10101!");
    });
  }

  @override
  Widget build(BuildContext context) {
    return AnnotatedRegion<SystemUiOverlayStyle>(
        value: SystemUiOverlayStyle.dark,
        child: Scaffold(
            backgroundColor: Colors.white,
            body: ScrollableSafeArea(
                child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              children: [
                Center(
                  child: Image.asset('assets/10101_logo_icon.png', width: 150, height: 150),
                ),
                const SizedBox(height: 40),
                const Center(child: CircularProgressIndicator()),
                const SizedBox(height: 15),
                Text(message)
              ],
            ))));
  }
}
