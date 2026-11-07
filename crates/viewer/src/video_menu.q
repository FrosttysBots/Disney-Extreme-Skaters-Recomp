// The viewer's Video Options, made as the game's own menus are (its
// make_new_skateshop_menu, main_menu_add_item and create_menu_bg, so its
// fonts and art): the window, its size, vsync and a frame limit. The
// viewer listens for viewer_video (an item chosen or turned, with
// `option` and `dir`; Apply, `option = video_apply`) and names each
// item's value (the item's text, its `text_id`). Nothing changes till
// Apply; leaving without it, nothing does.
//
// The game's create_menu_bg and main_menu_add_item are kept as
// viewer_disc_create_menu_bg and viewer_disc_main_menu_add_item, for
// these of the same names to call.

// The Skate Shop's Options: Video Options before Done...
script main_menu_add_item
    if GotParam id
        if (<id> = options_done)
            viewer_disc_main_menu_add_item {
                text = "Video Options"
                id = options_video
                pad_choose_script = options_menu_exit
                pad_choose_params = { new_menu_script = viewer_video_menu keep_ss_textures }
                highlight_bar_scale = (1.015, 1.3)
            }
        endif
    endif
    viewer_disc_main_menu_add_item <...>
endscript

// ... and its background a line taller for it.
script create_menu_bg
    if ObjectExists id = ss_setup_options_vmenu
        if NOT ObjectExists id = options_controller
            viewer_disc_create_menu_bg pos = <pos> num_items = 9
            return
        endif
    endif
    viewer_disc_create_menu_bg <...>
endscript

// The pause menu's Options: its Screen Mode is the Video Options.
script create_screen_menu
    viewer_video_menu
endscript

script viewer_video_menu
    if ObjectExists id = current_menu_anchor
        DestroyScreenElement id = current_menu_anchor
    endif
    if LevelIs load_skateshop
        <menu_pos> = (160.0, 89.0)
    else
        <menu_pos> = (320.0, 89.0)
    endif
    make_new_skateshop_menu {
        pos = <menu_pos>
        menu_just = [ center top ]
        internal_just = [ center center ]
        menu_id = video_menu
        vmenu_id = video_vmenu
        menu_title = "VIDEO"
        helper_text = generic_helper_text
    }
    create_menu_bg pos = (<menu_pos> + (0.0, 87.0)) num_items = 6
    SetScreenElementProps {
        id = video_vmenu
        event_handlers = [ { pad_back generic_menu_pad_back params = { callback = viewer_video_exit } } ]
        replace_handlers
    }
    viewer_video_item id = video_display text_id = video_display_text
    viewer_video_item id = video_size text_id = video_size_text
    viewer_video_item id = video_vsync text_id = video_vsync_text
    viewer_video_item id = video_fps text_id = video_fps_text
    main_menu_add_item {
        text = "Apply"
        id = video_apply
        pad_choose_script = viewer_video
        pad_choose_params = { option = video_apply }
        highlight_bar_scale = (1.015, 1.3)
    }
    main_menu_add_item {
        text = "Done"
        id = video_done
        pad_choose_script = viewer_video_exit
        highlight_bar_scale = (1.015, 1.3)
    }
    RunScriptOnScreenElement id = current_menu_anchor animate_in
endscript

// One setting: chosen or Right, the next value; Left, the one before.
script viewer_video_item
    main_menu_add_item {
        text = " "
        id = <id>
        text_id = <text_id>
        pad_choose_script = viewer_video
        pad_choose_params = { option = <id> dir = 1 }
        highlight_bar_scale = (1.015, 1.3)
    }
    SetScreenElementProps {
        id = <id>
        event_handlers = [
            { pad_left viewer_video params = { option = <id> dir = -1 } }
            { pad_right viewer_video params = { option = <id> dir = 1 } }
        ]
    }
endscript

script viewer_video_exit
    if ObjectExists id = current_menu_anchor
        DestroyScreenElement id = current_menu_anchor
    endif
    if LevelIs load_skateshop
        create_setup_options_menu
    else
        create_options_menu
    endif
endscript
