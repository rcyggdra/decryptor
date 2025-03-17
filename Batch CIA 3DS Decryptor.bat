@echo off
mode con cols=52 lines=26
title Batch CIA 3DS Decryptor
SetLocal EnableDelayedExpansion

echo 解密＆创建中...
echo.

:: 清理残留文件
del /q *.ncch >nul 2>&1

:: 处理所有 .3ds 文件
for %%a in (*.3ds) do (
    set CUTN=%%~na
    if /i x!CUTN!==x!CUTN:decrypted=! (
        bin\ctrtool "%%a" > content.txt
        set FILE="content.txt"

        :: 检查是否已解密，如果找到则跳过处理
        findstr /pr "Crypto.*None" !FILE! >nul 2>&1
        if errorlevel 1 (
            bin\ctrdecrypt "%%a" --no-verbose >nul 2>&1

        set ARG=
        for %%f in ("!CUTN!.*.ncch") do (
            set filename=%%~nf
            for /f "delims=" %%i in ("!filename!") do (
                set "last_dot=%%~xi"
            )
            set /a index=!last_dot:~1!
            set ARG=!ARG! -i "%%f:!index!:!index!"
        )

            :: 生成解密后的 .3ds 文件
            bin\makerom -f cci -ignoresign -target p -o "!CUTN!-decrypted.3ds"!ARG! >nul 2>&1
        ) else (
            echo 文件 "%%a" 已经解密，跳过处理。
            echo.
        )
    )
)

:: 处理所有 .cia 文件
for %%a in (*.cia) do (
    set CUTN=%%~na
    if /i x!CUTN!==x!CUTN:decrypted=! (
        bin\ctrtool "%%a" > content.txt
        set FILE="content.txt"

        :: 检查是否已解密，如果找到则跳过处理
        findstr /pr "Crypto.*None" !FILE! >nul 2>&1
        if errorlevel 1 (
            bin\ctrdecrypt "%%a" --no-verbose >nul 2>&1

            set ARG=
            set DLC=
            for %%f in ("!CUTN!.*.ncch") do (
                set filename=%%~nf

                for /f "delims=" %%i in ("!filename!") do (
                    set "last_dot=%%~xi"
                    set "filename=%%~ni"
                )

                for /f "delims=" %%i in ("!filename!") do (
                    set "second_last_dot=%%~xi"
                    set "filename=%%~ni"
                )

                set index=!second_last_dot:~1!
                set id=!last_dot:~1!

                for /f "tokens=* delims=0" %%d in ("!id!") do set "id=%%d"
                if "!id!"=="" set "id=0"
                set /a hex=0x!id!

                set ARG=!ARG! -i "%%f:!index!:!index!"
                set DLC=!DLC! -i "%%f:!index!:!hex!"
            )

            :: 获取版本信息
            for /f "tokens=2 delims=()" %%b in ('findstr "TitleVersion" !FILE!') do set "VER=%%b"

            :: 根据 TitleID 类型生成不同的 .cia 文件
            findstr /pr "^T.*d.*00040000" !FILE! >nul 2>&1
            if not errorlevel 1 (
                bin\makerom -f cia -ignoresign -target p -o "!CUTN!-decrypted.cia"!ARG! -ver !VER! >nul 2>&1
            )

            findstr /pr "^T.*d.*0004000e" !FILE! >nul 2>&1
            if not errorlevel 1 (
                bin\makerom -f cia -ignoresign -target p -o "!CUTN! (Update)-decrypted.cia"!DLC! -ver !VER! >nul 2>&1
            )

            findstr /pr "^T.*d.*0004008c" !FILE! >nul 2>&1
            if not errorlevel 1 (
                bin\dlchelper "!CUTN!" !VER! >nul 2>&1
            )
        ) else (
            echo 文件 "%%a" 已经解密，跳过处理。
            echo.
        )
    )
)

:: 清理临时文件
del content.txt >nul 2>&1
del /q *.ncch >nul 2>&1

echo 完成, 按任意键退出。
pause >nul 2>&1
exit